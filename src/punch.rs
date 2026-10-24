// Punch routes: what hbbs remembers about a controller connection whose
// PunchHoleRequest asked for UDP hole punching or WebRTC. The controlled
// side's answers, UDP PunchHoleSent and ICE candidates are only accepted for
// such a route, so they always belong to a request that passed the TCP checks
// (licence key, peer online). Routes are keyed by the controller's TCP address
// (the tcp_punch key), bounded in number, and expire.
use hbb_common::try_into_v4;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::Mutex,
    time::{Duration, Instant},
};

pub(crate) const MAX_SDP_LEN: usize = 4096;
pub(crate) const MAX_ICE_CANDIDATE_LEN: usize = 2048;
pub(crate) const MAX_SESSION_KEY_LEN: usize = 256;
// per route and direction; the controller sends each candidate twice
pub(crate) const MAX_ICE_PER_DIRECTION: u32 = 128;
// per TCP connection
pub(crate) const MAX_ICE_FRAMES: u32 = 256;
pub(crate) const MAX_ICE_MISSES: u32 = 16;
const MAX_ROUTES: usize = 10_000;
const MAX_ROUTES_PER_IP: usize = 64;
const UDP_ROUTE_TTL: Duration = Duration::from_secs(30);
const WEBRTC_ROUTE_TTL: Duration = Duration::from_secs(60);
const MAX_ROUTE_LIFETIME: Duration = Duration::from_secs(120);
// UDP TestNatRequest answers per source IP (IPv6: per /64) and in total
const TEST_NAT_BURST: f64 = 20.0;
const TEST_NAT_RATE: f64 = 10.0;
const TEST_NAT_GLOBAL_BURST: f64 = 4000.0;
const TEST_NAT_GLOBAL_RATE: f64 = 2000.0;
// IPv6 addresses a controller IP (IPv6: /64) may hand to peers
const IPV6_FORWARD_BURST: f64 = 60.0;
const IPV6_FORWARD_RATE: f64 = 1.0;
const LIMITER_SOURCES: usize = 65_536;

struct WebRtc {
    // DTLS fingerprint of the offer, the clients' ICE session key
    session_key: String,
    to_peer: u32,
    to_controller: u32,
}

struct Route {
    peer_id: String,
    // the controlled peer's registered IP when the route was opened
    peer_ip: IpAddr,
    created: Instant,
    expires: Instant,
    // Some(answered) once UDP punching was granted
    udp: Option<bool>,
    webrtc: Option<WebRtc>,
}

impl Route {
    fn alive(&self, now: Instant) -> bool {
        self.expires > now
    }
}

/// What a PunchHoleRequest may forward.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct Grant {
    pub udp: bool,
    pub webrtc: bool,
}

#[derive(Default)]
pub(crate) struct PunchRoutes {
    map: Mutex<HashMap<SocketAddr, Route>>,
}

impl PunchRoutes {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SocketAddr, Route>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens or refreshes the route of controller connection `ctl` to `peer_id`.
    /// A connection keeps the peer and the session key it started with.
    pub(crate) fn open(
        &self,
        ctl: SocketAddr,
        peer_id: &str,
        peer_ip: IpAddr,
        udp: bool,
        session_key: Option<String>,
    ) -> Grant {
        self.open_at(Instant::now(), ctl, peer_id, peer_ip, udp, session_key)
    }

    fn open_at(
        &self,
        now: Instant,
        ctl: SocketAddr,
        peer_id: &str,
        peer_ip: IpAddr,
        udp: bool,
        session_key: Option<String>,
    ) -> Grant {
        let ctl = try_into_v4(ctl);
        let mut grant = Grant::default();
        let mut map = self.lock();
        if map.get(&ctl).map(|r| !r.alive(now)).unwrap_or(false) {
            map.remove(&ctl);
        }
        if !map.contains_key(&ctl) {
            if !udp && session_key.is_none() {
                return grant;
            }
            let full = |map: &HashMap<SocketAddr, Route>| {
                map.len() >= MAX_ROUTES
                    || map.keys().filter(|k| k.ip() == ctl.ip()).count() >= MAX_ROUTES_PER_IP
            };
            if full(&map) {
                map.retain(|_, r| r.alive(now));
                if full(&map) {
                    return grant;
                }
            }
            map.insert(
                ctl,
                Route {
                    peer_id: peer_id.to_owned(),
                    peer_ip,
                    created: now,
                    expires: now,
                    udp: None,
                    webrtc: None,
                },
            );
        }
        let Some(r) = map.get_mut(&ctl) else {
            return grant;
        };
        if r.peer_id != peer_id {
            return grant;
        }
        r.peer_ip = peer_ip;
        if udp {
            r.udp.get_or_insert(false);
            grant.udp = true;
        }
        if let Some(key) = session_key {
            match &r.webrtc {
                None => {
                    r.webrtc = Some(WebRtc {
                        session_key: key,
                        to_peer: 0,
                        to_controller: 0,
                    });
                    grant.webrtc = true;
                }
                Some(w) => grant.webrtc = w.session_key == key,
            }
        }
        let ttl = if r.webrtc.is_some() {
            WEBRTC_ROUTE_TTL
        } else {
            UDP_ROUTE_TTL
        };
        r.expires = (now + ttl).min(r.created + MAX_ROUTE_LIFETIME);
        grant
    }

    /// While a WebRTC route is alive the controller's sink must survive the first
    /// response, so that the answer and later ICE candidates can reach it.
    pub(crate) fn keeps_sink(&self, ctl: SocketAddr) -> bool {
        let now = Instant::now();
        self.lock()
            .get(&try_into_v4(ctl))
            .map(|r| r.alive(now) && r.webrtc.is_some())
            .unwrap_or(false)
    }

    /// Whether a WebRTC answer from `peer_id` belongs to the route of `ctl`.
    pub(crate) fn webrtc_answer(&self, ctl: SocketAddr, peer_id: &str) -> bool {
        let now = Instant::now();
        self.lock()
            .get(&try_into_v4(ctl))
            .map(|r| r.alive(now) && r.webrtc.is_some() && r.peer_id == peer_id)
            .unwrap_or(false)
    }

    /// Counts a candidate from the controller on connection `ctl` for `peer_id`.
    pub(crate) fn ice_to_peer(&self, ctl: SocketAddr, peer_id: &str, session_key: &str) -> bool {
        self.ice_at(Instant::now(), ctl, Some(peer_id), session_key)
    }

    /// Counts a candidate from the controlled side for controller connection `ctl`.
    pub(crate) fn ice_to_controller(&self, ctl: SocketAddr, session_key: &str) -> bool {
        self.ice_at(Instant::now(), ctl, None, session_key)
    }

    fn ice_at(
        &self,
        now: Instant,
        ctl: SocketAddr,
        to_peer: Option<&str>,
        session_key: &str,
    ) -> bool {
        let mut map = self.lock();
        let Some(r) = map.get_mut(&try_into_v4(ctl)) else {
            return false;
        };
        if !r.alive(now) || to_peer.map(|id| id != r.peer_id).unwrap_or(false) {
            return false;
        }
        let Some(w) = r.webrtc.as_mut() else {
            return false;
        };
        if w.session_key != session_key {
            return false;
        }
        let n = if to_peer.is_some() {
            &mut w.to_peer
        } else {
            &mut w.to_controller
        };
        if *n >= MAX_ICE_PER_DIRECTION {
            return false;
        }
        *n += 1;
        true
    }

    /// Accepts the first UDP PunchHoleSent of the route of `ctl`, if it comes
    /// from `peer_id`'s address. Returns whether the route also carries WebRTC.
    pub(crate) fn claim_udp(&self, ctl: SocketAddr, peer_id: &str, src: SocketAddr) -> Option<bool> {
        self.claim_udp_at(Instant::now(), ctl, peer_id, src)
    }

    fn claim_udp_at(
        &self,
        now: Instant,
        ctl: SocketAddr,
        peer_id: &str,
        src: SocketAddr,
    ) -> Option<bool> {
        let mut map = self.lock();
        let r = map.get_mut(&try_into_v4(ctl))?;
        if !r.alive(now)
            || r.udp != Some(false)
            || r.peer_id != peer_id
            || !same_host(r.peer_ip, src.ip())
        {
            return None;
        }
        r.udp = Some(true);
        Some(r.webrtc.is_some())
    }

    pub(crate) fn remove(&self, ctl: SocketAddr) {
        self.lock().remove(&try_into_v4(ctl));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    fn full(now: Instant, burst: f64) -> Self {
        Self {
            tokens: burst,
            last: now,
        }
    }

    fn take(&mut self, now: Instant, rate: f64, burst: f64) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * rate).min(burst);
        self.last = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// Token buckets per source IP (IPv6: per /64), and optionally in total. When
/// the source table is full, unknown sources are refused.
pub(crate) struct RateLimiter {
    burst: f64,
    rate: f64,
    global: Option<(f64, f64)>,
    inner: Mutex<(Bucket, HashMap<IpAddr, Bucket>)>,
}

impl RateLimiter {
    fn new(burst: f64, rate: f64, global: Option<(f64, f64)>) -> Self {
        let global_burst = global.map(|g| g.0).unwrap_or_default();
        Self {
            burst,
            rate,
            global,
            inner: Mutex::new((Bucket::full(Instant::now(), global_burst), HashMap::new())),
        }
    }

    /// UDP TestNatRequest answers. Their source can be spoofed, so every answer
    /// is a packet to a possible victim.
    pub(crate) fn test_nat() -> Self {
        Self::new(
            TEST_NAT_BURST,
            TEST_NAT_RATE,
            Some((TEST_NAT_GLOBAL_BURST, TEST_NAT_GLOBAL_RATE)),
        )
    }

    /// IPv6 addresses forwarded from a controller: the peer sends punch probes
    /// to whatever address the controller names.
    pub(crate) fn ipv6_forward() -> Self {
        Self::new(IPV6_FORWARD_BURST, IPV6_FORWARD_RATE, None)
    }

    pub(crate) fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(Instant::now(), ip)
    }

    fn allow_at(&self, now: Instant, ip: IpAddr) -> bool {
        let source = match try_into_v4(SocketAddr::new(ip, 0)).ip() {
            IpAddr::V6(v6) => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
            ip => ip,
        };
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (global, sources) = &mut *guard;
        if !sources.contains_key(&source) && sources.len() >= LIMITER_SOURCES {
            // a bucket that has refilled is the same as none
            let refill = Duration::from_secs_f64(self.burst / self.rate);
            sources.retain(|_, b| now.saturating_duration_since(b.last) < refill);
            if sources.len() >= LIMITER_SOURCES {
                return false;
            }
        }
        let bucket = sources
            .entry(source)
            .or_insert_with(|| Bucket::full(now, self.burst));
        bucket.take(now, self.rate, self.burst)
            && self
                .global
                .map(|(burst, rate)| global.take(now, rate, burst))
                .unwrap_or(true)
    }
}

/// Whether a mangled IPv6 socket address is worth forwarding: a global unicast
/// address (2000::/3, what clients detect as public) with a port.
pub(crate) fn global_v6(bytes: &[u8]) -> bool {
    match decode_addr(bytes) {
        Some(SocketAddr::V6(addr)) if bytes.len() == 18 => {
            addr.port() != 0 && addr.ip().segments()[0] & 0xe000 == 0x2000
        }
        _ => false,
    }
}

/// Whether a client version ("1.5.0", "1.5.0-2") is at least `min`.
pub(crate) fn version_at_least(version: &str, min: (u32, u32, u32)) -> bool {
    let mut parts = version
        .split('-')
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|x| x.trim().parse::<u32>().unwrap_or(0));
    let mut next = || parts.next().unwrap_or(0);
    (next(), next(), next()) >= min
}

/// Whether a UDP answer to `addr` may be sent at all.
pub(crate) fn replyable(addr: SocketAddr) -> bool {
    let addr = try_into_v4(addr);
    let ip = addr.ip();
    addr.port() != 0
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !matches!(ip, IpAddr::V4(v4) if v4.is_broadcast())
}

/// Whether two addresses belong to the same host: equal IPv4, or the same IPv6 /64
/// (privacy extensions give each socket another address in the prefix).
pub(crate) fn same_host(a: IpAddr, b: IpAddr) -> bool {
    let norm = |ip: IpAddr| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        ip => ip,
    };
    match (norm(a), norm(b)) {
        (IpAddr::V4(a), IpAddr::V4(b)) => a == b,
        (IpAddr::V6(a), IpAddr::V6(b)) => a.segments()[..4] == b.segments()[..4],
        _ => false,
    }
}

/// AddrMangle::decode without its unchecked arithmetic, for addresses from the
/// network. Same result as AddrMangle::decode in a release build.
pub(crate) fn decode_addr(bytes: &[u8]) -> Option<SocketAddr> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.len() > 16 {
        if bytes.len() != 18 {
            return None;
        }
        let mut ip = [0u8; 16];
        ip.copy_from_slice(&bytes[..16]);
        let port = u16::from_le_bytes([bytes[16], bytes[17]]);
        return Some(SocketAddr::V6(SocketAddrV6::new(ip.into(), port, 0, 0)));
    }
    let mut padded = [0u8; 16];
    padded[..bytes.len()].copy_from_slice(bytes);
    let number = u128::from_le_bytes(padded);
    let tm = (number >> 17) & (u32::MAX as u128);
    let ip = ((number >> 49).wrapping_sub(tm) as u32).to_le_bytes();
    let port = (number & 0xFF_FFFF).wrapping_sub(tm & 0xFFFF);
    Some(SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]),
        port as u16,
    )))
}

/// The clients' ICE session key for a WebRTC offer: the DTLS fingerprint of its
/// SDP (hbb_common webrtc.rs `get_key_for_sdp`). The offer is "webrtc://" +
/// base64 of a JSON session description.
pub(crate) fn offer_session_key(offer: &str) -> Option<String> {
    if offer.len() > MAX_SDP_LEN {
        return None;
    }
    let json = base64::decode(offer.strip_prefix("webrtc://")?).ok()?;
    let desc: serde_json::Value = serde_json::from_slice(&json).ok()?;
    let key = sdp_fingerprint(desc.get("sdp")?.as_str()?)?;
    if key.is_empty() || key.len() > MAX_SESSION_KEY_LEN {
        return None;
    }
    Some(key)
}

// The session-level a=fingerprint, else the first one of an application section.
fn sdp_fingerprint(sdp: &str) -> Option<String> {
    let mut in_media = false;
    let mut application = false;
    let mut media_fp = None;
    for line in sdp.lines() {
        if let Some(media) = line.strip_prefix("m=") {
            in_media = true;
            application = media.split(' ').next() == Some("application");
        } else if let Some(fp) = line.strip_prefix("a=fingerprint:") {
            if !in_media {
                return Some(fp.to_owned());
            }
            if application && media_fp.is_none() {
                media_fp = Some(fp.to_owned());
            }
        }
    }
    media_fp
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbb_common::AddrMangle;

    const FP: &str = "sha-256 97:52:D6:1F:1E:87:6C:DA:B8:21:95:64:A5:85:89:FA:02:71:C7:4D:B3:FD:25:92:40:FB:6B:65:24:3C:79:88";

    fn ctl(port: u16) -> SocketAddr {
        SocketAddr::new("10.0.0.1".parse().unwrap(), port)
    }

    fn peer_ip() -> IpAddr {
        "10.0.0.2".parse().unwrap()
    }

    pub(crate) fn offer(sdp: &str) -> String {
        let json = serde_json::json!({"type": "offer", "sdp": sdp, "ice_policy": "all"});
        format!("webrtc://{}", base64::encode(json.to_string()))
    }

    #[test]
    fn session_key_from_session_level_or_application_section() {
        let session = format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=fingerprint:{}\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=fingerprint:sha-256 00\r\n",
            FP
        );
        assert_eq!(offer_session_key(&offer(&session)).as_deref(), Some(FP));
        let media = format!(
            "v=0\r\ns=-\r\nt=0 0\r\nm=audio 9 RTP/AVP 0\r\na=fingerprint:sha-256 11\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=fingerprint:{}\r\n",
            FP
        );
        assert_eq!(offer_session_key(&offer(&media)).as_deref(), Some(FP));
        assert_eq!(offer_session_key(&offer("v=0\r\ns=-\r\n")), None);
        assert_eq!(offer_session_key("webrtc://not base64!"), None);
        assert_eq!(offer_session_key(&format!("webrtc://{}", base64::encode("[1,2]"))), None);
        assert_eq!(offer_session_key(&offer(&session).replace("webrtc://", "x://")), None);
        let big = format!("v=0\r\na=fingerprint:{}\r\na=x:{}\r\n", FP, "y".repeat(MAX_SDP_LEN));
        assert_eq!(offer_session_key(&offer(&big)), None);
    }

    #[test]
    fn decode_matches_addr_mangle_and_never_panics() {
        for addr in ["1.2.3.4:5", "255.255.255.255:65535", "0.0.0.0:0", "[2001:db8::1]:443"] {
            let addr: SocketAddr = addr.parse().unwrap();
            assert_eq!(decode_addr(&AddrMangle::encode(addr)), Some(addr));
        }
        assert_eq!(decode_addr(&[]), None);
        assert_eq!(decode_addr(&[1u8; 17]), None);
        assert_eq!(decode_addr(&[1u8; 40]), None);
        for n in 1..=16 {
            decode_addr(&vec![0xffu8; n]);
            decode_addr(&vec![0x01u8; n]);
        }
    }

    #[test]
    fn same_host_compares_v4_and_v6_prefixes() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(same_host(ip("1.2.3.4"), ip("::ffff:1.2.3.4")));
        assert!(!same_host(ip("1.2.3.4"), ip("1.2.3.5")));
        assert!(same_host(ip("2001:db8:1:2::1"), ip("2001:db8:1:2:aaaa::9")));
        assert!(!same_host(ip("2001:db8:1:2::1"), ip("2001:db8:1:3::1")));
        assert!(!same_host(ip("1.2.3.4"), ip("2001:db8::1")));
    }

    #[test]
    fn routes_need_a_purpose_and_keep_their_peer() {
        let routes = PunchRoutes::default();
        assert_eq!(routes.open(ctl(1), "peer01", peer_ip(), false, None), Grant::default());
        assert_eq!(routes.len(), 0);
        let g = routes.open(ctl(1), "peer01", peer_ip(), true, Some(FP.to_owned()));
        assert_eq!(g, Grant { udp: true, webrtc: true });
        // the same connection cannot switch to another peer or session
        assert_eq!(
            routes.open(ctl(1), "peer02", peer_ip(), true, Some(FP.to_owned())),
            Grant::default()
        );
        assert_eq!(
            routes.open(ctl(1), "peer01", peer_ip(), false, Some("other".to_owned())),
            Grant { udp: false, webrtc: false }
        );
        assert!(routes.keeps_sink(ctl(1)));
        assert!(routes.webrtc_answer(ctl(1), "peer01"));
        assert!(!routes.webrtc_answer(ctl(1), "peer02"));
        routes.remove(ctl(1));
        assert!(!routes.keeps_sink(ctl(1)));
    }

    #[test]
    fn ice_needs_the_session_key_and_has_a_budget() {
        let routes = PunchRoutes::default();
        routes.open(ctl(1), "peer01", peer_ip(), false, Some(FP.to_owned()));
        assert!(!routes.ice_to_peer(ctl(1), "peer01", "sha-256 00"));
        assert!(!routes.ice_to_peer(ctl(1), "peer02", FP));
        assert!(!routes.ice_to_controller(ctl(2), FP));
        for _ in 0..MAX_ICE_PER_DIRECTION {
            assert!(routes.ice_to_peer(ctl(1), "peer01", FP));
            assert!(routes.ice_to_controller(ctl(1), FP));
        }
        assert!(!routes.ice_to_peer(ctl(1), "peer01", FP));
        assert!(!routes.ice_to_controller(ctl(1), FP));
        // a UDP-only route carries no ICE and keeps no sink
        routes.open(ctl(3), "peer01", peer_ip(), true, None);
        assert!(!routes.ice_to_controller(ctl(3), FP));
        assert!(!routes.keeps_sink(ctl(3)));
    }

    #[test]
    fn routes_expire() {
        let routes = PunchRoutes::default();
        let t0 = Instant::now();
        routes.open_at(t0, ctl(1), "peer01", peer_ip(), true, None);
        routes.open_at(t0, ctl(2), "peer01", peer_ip(), false, Some(FP.to_owned()));
        let src = SocketAddr::new(peer_ip(), 5000);
        assert_eq!(routes.claim_udp_at(t0 + UDP_ROUTE_TTL, ctl(1), "peer01", src), None);
        assert!(!routes.ice_at(t0 + WEBRTC_ROUTE_TTL, ctl(2), None, FP));
        // refreshing never extends a route past its lifetime
        let mut t = t0;
        while t < t0 + MAX_ROUTE_LIFETIME {
            routes.open_at(t, ctl(2), "peer01", peer_ip(), false, Some(FP.to_owned()));
            t += Duration::from_secs(10);
        }
        assert!(!routes.ice_at(t0 + MAX_ROUTE_LIFETIME, ctl(2), None, FP));
        // an expired route is replaced by a new request
        let g = routes.open_at(t0 + MAX_ROUTE_LIFETIME, ctl(2), "peer02", peer_ip(), true, None);
        assert!(g.udp);
    }

    #[test]
    fn udp_claim_is_single_and_checks_the_sender() {
        let routes = PunchRoutes::default();
        routes.open(ctl(1), "peer01", peer_ip(), true, None);
        let other = SocketAddr::new("10.9.9.9".parse().unwrap(), 5000);
        assert_eq!(routes.claim_udp(ctl(1), "peer01", other), None);
        assert_eq!(routes.claim_udp(ctl(1), "peer02", SocketAddr::new(peer_ip(), 1)), None);
        assert_eq!(routes.claim_udp(ctl(1), "peer01", SocketAddr::new(peer_ip(), 5000)), Some(false));
        assert_eq!(routes.claim_udp(ctl(1), "peer01", SocketAddr::new(peer_ip(), 5000)), None);
        // no UDP granted, no claim
        routes.open(ctl(2), "peer01", peer_ip(), false, Some(FP.to_owned()));
        assert_eq!(routes.claim_udp(ctl(2), "peer01", SocketAddr::new(peer_ip(), 1)), None);
    }

    #[test]
    fn test_nat_is_limited_per_source_and_in_total() {
        let limiter = RateLimiter::test_nat();
        let t0 = Instant::now();
        let a: IpAddr = "1.2.3.4".parse().unwrap();
        let n = (0..100).filter(|_| limiter.allow_at(t0, a)).count();
        assert_eq!(n, TEST_NAT_BURST as usize);
        // the mapped form is the same source
        assert!(!limiter.allow_at(t0, "::ffff:1.2.3.4".parse().unwrap()));
        assert!(limiter.allow_at(t0, "1.2.3.5".parse().unwrap()));
        // refills at the rate
        let t1 = t0 + Duration::from_millis(500);
        let n = (0..100).filter(|_| limiter.allow_at(t1, a)).count();
        assert_eq!(n, (TEST_NAT_RATE / 2.0) as usize);
        // a /64 is one source
        let v6 = |s: &str| s.parse::<IpAddr>().unwrap();
        let n = (0..100)
            .filter(|i| limiter.allow_at(t0, v6(&format!("2001:db8::{:x}", i + 1))))
            .count();
        assert_eq!(n, TEST_NAT_BURST as usize);
        assert!(limiter.allow_at(t0, v6("2001:db8:0:1::1")));
        // the global bucket caps all sources together
        let limiter = RateLimiter::test_nat();
        let mut n = 0;
        for i in 0..(TEST_NAT_GLOBAL_BURST as u32 + 100) {
            if limiter.allow_at(t0, IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i))) {
                n += 1;
            }
        }
        assert_eq!(n, TEST_NAT_GLOBAL_BURST as u32);
    }

    #[test]
    fn test_nat_source_table_is_bounded() {
        let limiter = RateLimiter::test_nat();
        let t0 = Instant::now();
        for i in 0..LIMITER_SOURCES as u32 {
            limiter.allow_at(t0, IpAddr::V4(Ipv4Addr::from(0x0b00_0000 + i)));
        }
        let len = |l: &RateLimiter| l.inner.lock().unwrap().1.len();
        assert_eq!(len(&limiter), LIMITER_SOURCES);
        // full: a new source is refused until old buckets have refilled
        assert!(!limiter.allow_at(t0, "1.1.1.1".parse().unwrap()));
        assert_eq!(len(&limiter), LIMITER_SOURCES);
        assert!(limiter.allow_at(t0 + Duration::from_secs(3), "1.1.1.1".parse().unwrap()));
        assert_eq!(len(&limiter), 1);
    }

    #[test]
    fn ipv6_forwards_are_limited_per_controller() {
        let limiter = RateLimiter::ipv6_forward();
        let t0 = Instant::now();
        let a: IpAddr = "1.2.3.4".parse().unwrap();
        let n = (0..100).filter(|_| limiter.allow_at(t0, a)).count();
        assert_eq!(n, IPV6_FORWARD_BURST as usize);
        assert!(limiter.allow_at(t0 + Duration::from_secs(1), a));
        // no global cap
        for i in 0..1000u32 {
            assert!(limiter.allow_at(t0, IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i))));
        }
    }

    #[test]
    fn only_global_v6_with_a_port_is_forwarded() {
        let m = |s: &str| AddrMangle::encode(s.parse::<SocketAddr>().unwrap());
        assert!(global_v6(&m("[2001:db8::1]:40000")));
        assert!(global_v6(&m("[2a00:1450::5]:1")));
        for bad in ["[2001:db8::1]:0", "[::1]:5", "[fe80::1]:5", "[fd00::1]:5", "[ff02::1]:5", "[64:ff9b::102:304]:5"] {
            assert!(!global_v6(&m(bad)), "{}", bad);
        }
        // a v4-mapped address in the raw 18-byte form, and a mangled IPv4
        let mut mapped = "::ffff:1.2.3.4".parse::<Ipv6Addr>().unwrap().octets().to_vec();
        mapped.extend_from_slice(&5u16.to_le_bytes());
        assert!(!global_v6(&mapped));
        assert!(!global_v6(&m("1.2.3.4:5")));
        assert!(!global_v6(&[0x20u8; 17]));
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(version_at_least("1.5.0", (1, 5, 0)));
        assert!(version_at_least("1.5.1", (1, 5, 0)));
        assert!(version_at_least("1.10.0", (1, 5, 0)));
        assert!(version_at_least("2.0", (1, 5, 0)));
        assert!(version_at_least("1.5.0-3", (1, 5, 0)));
        assert!(!version_at_least("1.4.9", (1, 5, 0)));
        assert!(!version_at_least("", (1, 5, 0)));
        assert!(!version_at_least("abc", (1, 5, 0)));
        assert!(!version_at_least("99999999999999999999.1", (1, 5, 0)));
        assert!(!version_at_least("1.1.1.1.1.1.1.1.1.1.1.1.1.1.1.1.1", (1, 5, 0)));
    }

    #[test]
    fn replyable_skips_unusable_addresses() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(replyable(a("1.2.3.4:5")));
        assert!(replyable(a("[2001:db8::1]:5")));
        assert!(!replyable(a("1.2.3.4:0")));
        assert!(!replyable(a("0.0.0.0:5")));
        assert!(!replyable(a("255.255.255.255:5")));
        assert!(!replyable(a("224.0.0.1:5")));
        assert!(!replyable(a("[::ffff:255.255.255.255]:5")));
        assert!(!replyable(a("[ff02::1]:5")));
    }

    #[test]
    fn routes_are_bounded() {
        let routes = PunchRoutes::default();
        for port in 0..MAX_ROUTES_PER_IP as u16 {
            assert!(routes.open(ctl(port), "peer01", peer_ip(), true, None).udp);
        }
        assert!(!routes.open(ctl(9999), "peer01", peer_ip(), true, None).udp);
        let mut n = MAX_ROUTES_PER_IP;
        'fill: for a in 0..=255u8 {
            for b in 0..=255u8 {
                if n >= MAX_ROUTES {
                    break 'fill;
                }
                let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 1, a, b)), 1);
                assert!(routes.open(addr, "peer01", peer_ip(), true, None).udp);
                n += 1;
            }
        }
        assert_eq!(routes.len(), MAX_ROUTES);
        let late = SocketAddr::new("10.2.0.1".parse().unwrap(), 1);
        assert!(!routes.open(late, "peer01", peer_ip(), true, None).udp);
        // expired routes make room again
        let later = Instant::now() + MAX_ROUTE_LIFETIME;
        assert!(routes.open_at(later, late, "peer01", peer_ip(), true, None).udp);
    }
}
