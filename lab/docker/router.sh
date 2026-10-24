#!/bin/sh
# Home-router emulation (needs NET_ADMIN and forwarding sysctls):
#   IPv4: LAN -> WAN MASQUERADE (Linux conntrack NAT, port-preserving where possible).
#   IPv6: routed, no NAT, stateful firewall (only replies come in), like most home routers.
# Environment:
#   WAN_NET4   WAN IPv4 prefix, used to find the WAN interface (e.g. 198.18.10.)
#   LAN_V4     LAN IPv4 subnet (e.g. 192.168.11.0/24)
#   LAN_V6     LAN IPv6 subnet (e.g. 2001:db8:11::/64)
#   ROUTES6    ";"-separated "prefix via addr" routes to the other LANs' v6 prefixes
#   V6_FIREWALL  1 (default) = drop unsolicited inbound IPv6, 0 = open
#   WAN_INPUT  drop (default) | reject: what the router itself does with unsolicited
#              packets to its WAN address. Home routers silently drop; "reject" (kernel
#              default: TCP RST / ICMP port unreachable) kills TCP hole punching at once.
# With "routes" as first argument it only adds ROUTES6 and exits (used as a helper that
# shares the network namespace of a WAN container such as hbbs).
set -eu
add_routes() {
    echo "${ROUTES6:-}" | tr ';' '\n' | while read -r r; do
        if [ -n "$r" ]; then ip -6 route replace $r; fi
    done
    ip -6 route
}
if [ "${1:-}" = routes ]; then
    add_routes
    exit 0
fi
WAN=$(ip -o -4 addr show | awk -v p="${WAN_NET4:?}" 'index($4, p) == 1 {print $2}' | head -1)
LAN=$(ip -o -4 addr show | awk -v n="${LAN_V4:?}" 'BEGIN{split(n, a, "."); p=a[1]"."a[2]"."a[3]"."} index($4, p) == 1 {print $2}' | head -1)
echo "rdlab-router: wan=$WAN lan=$LAN lan_v4=$LAN_V4 lan_v6=${LAN_V6:-}"
[ -n "$WAN" ] && [ -n "$LAN" ] || { echo "interfaces not found"; ip -o addr; exit 1; }

iptables -t nat -A POSTROUTING -s "$LAN_V4" -o "$WAN" -j MASQUERADE
iptables -P FORWARD DROP
# explicit (counted) INVALID drop: conntrack marks e.g. a SYN-ACK whose sequence number does
# not match the SYN it saw as INVALID (see nf_conntrack_tcp_be_liberal)
iptables -A FORWARD -m conntrack --ctstate INVALID -j DROP
iptables -A FORWARD -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
iptables -A FORWARD -i "$LAN" -o "$WAN" -j ACCEPT

ip6tables -P FORWARD DROP
ip6tables -A FORWARD -m conntrack --ctstate INVALID -j DROP
ip6tables -A FORWARD -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
ip6tables -A FORWARD -i "$LAN" -o "$WAN" -j ACCEPT
ip6tables -A FORWARD -p ipv6-icmp --icmpv6-type echo-request -j ACCEPT
if [ "${V6_FIREWALL:-1}" != 1 ]; then
    ip6tables -A FORWARD -i "$WAN" -o "$LAN" -j ACCEPT
fi
if [ "${WAN_INPUT:-drop}" = drop ]; then
    iptables -A INPUT -i "$WAN" -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    iptables -A INPUT -i "$WAN" -p icmp -j ACCEPT
    iptables -A INPUT -i "$WAN" -j DROP
    ip6tables -A INPUT -i "$WAN" -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    ip6tables -A INPUT -i "$WAN" -p ipv6-icmp -j ACCEPT
    ip6tables -A INPUT -i "$WAN" -j DROP
fi
add_routes
iptables -S; iptables -t nat -S; ip6tables -S
echo "rdlab-router: ready"
echo "nf_conntrack_tcp_be_liberal=$(cat /proc/sys/net/netfilter/nf_conntrack_tcp_be_liberal 2>/dev/null)"
trap 'exit 0' TERM INT
while :; do sleep 3600 & wait $!; done
