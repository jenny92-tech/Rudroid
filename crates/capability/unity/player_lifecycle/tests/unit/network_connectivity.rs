//! INPUT:  IPv4/IPv6 default routes, malformed rows and current host link files
//! OUTPUT:  route validation and opt-in real-device reachability evidence
//! POS:    safe query tests; a local route does not establish Internet/server availability
use super::*;
#[test]
fn only_up_non_loopback_default_routes_are_candidates() {
    let good = "wlan0 00000000 010A0A0A 0003 0 0 1 00000000 0 0 0";
    assert_eq!(default_interfaces(good, false), vec!["wlan0"]);
    for bad in [
        good.replace("0003", "0000"),
        good.replace("0003", "0201"),
        good.replace("wlan0", "lo"),
        good.replace("wlan0", "../../tmp"),
        good.replace("00000000", "00FFFFFF"),
        "incomplete".into(),
    ] {
        assert!(default_interfaces(&bad, false).is_empty());
    }
    let v6="00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000001 00000000 00000000 00000003 eth0";
    assert_eq!(default_interfaces(v6, true), vec!["eth0"]);
    assert!(default_interfaces(&v6.replacen(" 00 ", " 40 ", 1), true).is_empty());
}
#[test]
#[ignore = "run on the connected handheld after confirming its default route"]
fn connected_handheld_reports_the_apk_local_network_value() {
    assert_eq!(detected(), 2);
    assert_eq!(current(), 2);
}
