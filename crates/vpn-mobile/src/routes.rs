//! Pure IPv4 route planning for explicitly configured LAN exclusions.
//!
//! The caller selects exclusions after matching physical interface addresses.
//! Planning is atomic: an invalid or oversized plan returns no partial routes.
//! This module never reads or changes the host routing table.

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use thiserror::Error;

/// Bound both OS route churn and the expansion caused by CIDR subtraction.
pub const MAX_EFFECTIVE_ROUTES: usize = 1024;
const MAX_INPUT_NETWORKS: usize = 4096;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RouteBypassError {
    #[error("allowed route at index {index} is not a valid IPv4 CIDR")]
    InvalidAllowedRoute { index: usize },
    #[error("allowed route at index {index} is a default route")]
    DefaultAllowedRoute { index: usize },
    #[error("exclusion at index {index} is a default route")]
    DefaultExclusion { index: usize },
    #[error("the VPN subnet must not be a default route")]
    DefaultVpnSubnet,
    #[error("{kind} contains more than {limit} networks")]
    TooManyInputs { kind: &'static str, limit: usize },
    #[error("the effective route plan exceeds {limit} routes")]
    TooManyRoutes { limit: usize },
}

/// Compute `(allowed - exclusions) ∪ vpn`, retaining an explicit VPN route.
///
/// Networks are normalized and overlapping inputs are merged before subtraction.
/// A more specific exclusion splits a containing route into the remaining CIDRs;
/// leaving that containing route intact would send excluded traffic to the VPN.
/// The VPN subnet itself is always retained, even if an exclusion covers it.
///
/// Invalid/IPv6/default allowed routes and default exclusions are rejected as a
/// complete plan. Callers must retain or restore their complete safe route plan
/// on error, rather than interpreting an error as an empty route list.
pub fn effective_routes(
    allowed: &[String],
    vpn: Ipv4Net,
    exclusions: &[Ipv4Net],
) -> Result<Vec<Ipv4Net>, RouteBypassError> {
    if vpn.prefix_len() == 0 {
        return Err(RouteBypassError::DefaultVpnSubnet);
    }
    check_input_count("allowed routes", allowed.len())?;
    check_input_count("exclusions", exclusions.len())?;

    let mut allowed_ranges = Vec::with_capacity(allowed.len());
    for (index, value) in allowed.iter().enumerate() {
        let network = value
            .trim()
            .parse::<Ipv4Net>()
            .map_err(|_| RouteBypassError::InvalidAllowedRoute { index })?;
        if network.prefix_len() == 0 {
            return Err(RouteBypassError::DefaultAllowedRoute { index });
        }
        allowed_ranges.push(AddressRange::from(network));
    }
    let mut excluded_ranges = Vec::with_capacity(exclusions.len());
    for (index, &network) in exclusions.iter().enumerate() {
        if network.prefix_len() == 0 {
            return Err(RouteBypassError::DefaultExclusion { index });
        }
        excluded_ranges.push(AddressRange::from(network));
    }

    let allowed_ranges = merged(allowed_ranges);
    let excluded_ranges = merged(excluded_ranges);
    let mut routes = Vec::new();
    let mut first_exclusion = 0;
    for allowed in allowed_ranges {
        let mut cursor = allowed.start;
        while first_exclusion < excluded_ranges.len()
            && excluded_ranges[first_exclusion].end <= cursor
        {
            first_exclusion += 1;
        }
        for excluded in &excluded_ranges[first_exclusion..] {
            if excluded.start >= allowed.end {
                break;
            }
            if excluded.start > cursor {
                append_cidrs(cursor, excluded.start, &mut routes)?;
            }
            cursor = cursor.max(excluded.end).min(allowed.end);
            if cursor == allowed.end {
                break;
            }
        }
        if cursor < allowed.end {
            append_cidrs(cursor, allowed.end, &mut routes)?;
        }
    }

    let vpn = vpn.trunc();
    if !routes.contains(&vpn) {
        push_route(vpn, &mut routes)?;
    }
    routes.sort_unstable_by_key(|route| (u32::from(route.network()), route.prefix_len()));
    Ok(routes)
}

fn check_input_count(kind: &'static str, count: usize) -> Result<(), RouteBypassError> {
    if count > MAX_INPUT_NETWORKS {
        return Err(RouteBypassError::TooManyInputs {
            kind,
            limit: MAX_INPUT_NETWORKS,
        });
    }
    Ok(())
}

/// Half-open bounds use u64 so the address after 255.255.255.255 is representable.
#[derive(Clone, Copy, Debug)]
struct AddressRange {
    start: u64,
    end: u64,
}

impl From<Ipv4Net> for AddressRange {
    fn from(network: Ipv4Net) -> Self {
        Self {
            start: u64::from(u32::from(network.network())),
            end: u64::from(u32::from(network.broadcast())) + 1,
        }
    }
}

fn merged(mut ranges: Vec<AddressRange>) -> Vec<AddressRange> {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut result: Vec<AddressRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(previous) = result.last_mut() {
            if range.start <= previous.end {
                previous.end = previous.end.max(range.end);
                continue;
            }
        }
        result.push(range);
    }
    result
}

fn append_cidrs(
    mut start: u64,
    end: u64,
    routes: &mut Vec<Ipv4Net>,
) -> Result<(), RouteBypassError> {
    while start < end {
        let aligned_bits = start.trailing_zeros().min(32);
        let remaining_bits = 63 - (end - start).leading_zeros();
        // Adjacent valid inputs can cover all IPv4. Preserve two /1 routes in
        // that case instead of introducing a /0 route the caller did not allow.
        let host_bits = aligned_bits.min(remaining_bits).min(31);
        let network = Ipv4Net::new(Ipv4Addr::from(start as u32), (32 - host_bits) as u8)
            .expect("computed IPv4 prefix is in 1..=32");
        push_route(network, routes)?;
        start += 1_u64 << host_bits;
    }
    Ok(())
}

fn push_route(route: Ipv4Net, routes: &mut Vec<Ipv4Net>) -> Result<(), RouteBypassError> {
    if routes.len() == MAX_EFFECTIVE_ROUTES {
        return Err(RouteBypassError::TooManyRoutes {
            limit: MAX_EFFECTIVE_ROUTES,
        });
    }
    routes.push(route);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(value: &str) -> Ipv4Net {
        value.parse().unwrap()
    }

    fn plan(allowed: &[&str], excluded: &[&str]) -> Vec<Ipv4Net> {
        effective_routes(
            &allowed
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
            net("192.168.133.0/24"),
            &excluded.iter().map(|value| net(value)).collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn covers(routes: &[Ipv4Net], address: &str) -> bool {
        let address: Ipv4Addr = address.parse().unwrap();
        routes.iter().any(|route| route.contains(&address))
    }

    #[test]
    fn removing_a_subnet_splits_its_containing_route() {
        let routes = plan(&["10.0.0.0/8"], &["10.1.0.0/16"]);
        assert_eq!(
            routes,
            [
                "10.0.0.0/16",
                "10.2.0.0/15",
                "10.4.0.0/14",
                "10.8.0.0/13",
                "10.16.0.0/12",
                "10.32.0.0/11",
                "10.64.0.0/10",
                "10.128.0.0/9",
                "192.168.133.0/24",
            ]
            .map(net)
        );
        assert!(!covers(&routes, "10.1.255.255"));
        assert!(covers(&routes, "10.0.255.255"));
        assert!(covers(&routes, "10.2.0.0"));
    }

    #[test]
    fn equal_and_covering_exclusions_remove_only_allowed_networks() {
        let routes = plan(
            &["10.1.0.0/16", "172.16.0.0/16", "192.168.188.0/24"],
            &["10.0.0.0/8", "172.16.0.0/16", "203.0.113.0/24"],
        );
        assert_eq!(routes, [net("192.168.133.0/24"), net("192.168.188.0/24")]);
    }

    #[test]
    fn overlapping_inputs_normalize_merge_and_deduplicate() {
        let routes = plan(
            &[
                " 10.0.0.23/24 ",
                "10.0.0.128/25",
                "10.0.0.0/24",
                "10.0.1.0/24",
            ],
            &["10.0.0.70/26", "10.0.0.0/25", "10.0.0.32/27"],
        );
        assert_eq!(
            routes,
            [
                net("10.0.0.128/25"),
                net("10.0.1.0/24"),
                net("192.168.133.0/24")
            ]
        );
    }

    #[test]
    fn no_active_exclusions_restores_complete_allowed_coverage() {
        let routes = plan(&["10.0.0.0/8", "192.168.188.123/24"], &[]);
        assert_eq!(
            routes,
            [
                net("10.0.0.0/8"),
                net("192.168.133.0/24"),
                net("192.168.188.0/24")
            ]
        );
    }

    #[test]
    fn vpn_route_is_explicitly_retained_even_when_excluded_or_absent() {
        for allowed in [vec![], vec!["192.168.0.0/16"], vec!["192.168.133.0/24"]] {
            assert_eq!(
                plan(&allowed, &["192.168.0.0/16"]),
                [net("192.168.133.0/24")]
            );
        }
        let routes = plan(&["192.168.0.0/16"], &[]);
        assert_eq!(routes, [net("192.168.0.0/16"), net("192.168.133.0/24")]);
    }

    #[test]
    fn vpn_is_normalized_without_duplicate_entries() {
        let routes = effective_routes(
            &["192.168.133.123/24".to_owned()],
            net("192.168.133.7/24"),
            &[],
        )
        .unwrap();
        assert_eq!(routes, [net("192.168.133.0/24")]);
    }

    #[test]
    fn host_routes_and_last_ipv4_address_do_not_overflow() {
        assert_eq!(
            plan(&["255.255.255.252/30"], &["255.255.255.253/32"]),
            [
                net("192.168.133.0/24"),
                net("255.255.255.252/32"),
                net("255.255.255.254/31")
            ]
        );
        assert!(covers(
            &plan(&["255.255.255.255/32"], &[]),
            "255.255.255.255"
        ));
    }

    #[test]
    fn merged_inputs_never_introduce_a_default_route() {
        let routes = plan(&["0.0.0.0/1", "128.0.0.0/1"], &[]);
        assert_eq!(
            routes,
            [
                net("0.0.0.0/1"),
                net("128.0.0.0/1"),
                net("192.168.133.0/24")
            ]
        );
        assert!(routes.iter().all(|route| route.prefix_len() > 0));
    }

    #[test]
    fn invalid_ipv6_and_default_inputs_reject_the_entire_plan() {
        for bad in ["not-a-network", "10.0.0.0/33", "::1/128", ""] {
            assert_eq!(
                effective_routes(
                    &["10.0.0.0/8".to_owned(), bad.to_owned()],
                    net("192.168.133.0/24"),
                    &[]
                ),
                Err(RouteBypassError::InvalidAllowedRoute { index: 1 })
            );
        }
        assert_eq!(
            effective_routes(&["123.4.5.6/0".to_owned()], net("192.168.133.0/24"), &[]),
            Err(RouteBypassError::DefaultAllowedRoute { index: 0 })
        );
        assert_eq!(
            effective_routes(&[], net("192.168.133.0/24"), &[net("10.0.0.0/0")]),
            Err(RouteBypassError::DefaultExclusion { index: 0 })
        );
        assert_eq!(
            effective_routes(&[], net("0.0.0.0/0"), &[]),
            Err(RouteBypassError::DefaultVpnSubnet)
        );
    }

    #[test]
    fn route_limit_includes_the_mandatory_vpn_route() {
        let allowed: Vec<_> = (0..MAX_EFFECTIVE_ROUTES)
            .map(|index| format!("{}/32", Ipv4Addr::from(0x0a00_0000 + index as u32 * 2)))
            .collect();
        assert_eq!(
            effective_routes(&allowed, net("192.168.133.0/24"), &[]),
            Err(RouteBypassError::TooManyRoutes {
                limit: MAX_EFFECTIVE_ROUTES
            })
        );
        let routes =
            effective_routes(&allowed[..allowed.len() - 1], net("192.168.133.0/24"), &[]).unwrap();
        assert_eq!(routes.len(), MAX_EFFECTIVE_ROUTES);
    }

    #[test]
    fn exclusion_expansion_is_bounded() {
        let exclusions: Vec<_> = (0..1024)
            .map(|index| Ipv4Net::new(Ipv4Addr::from(0x0a00_0001 + index * 256), 32).unwrap())
            .collect();
        assert_eq!(
            effective_routes(
                &["10.0.0.0/8".to_owned()],
                net("192.168.133.0/24"),
                &exclusions
            ),
            Err(RouteBypassError::TooManyRoutes {
                limit: MAX_EFFECTIVE_ROUTES
            })
        );
    }

    #[test]
    fn input_counts_are_bounded_before_processing() {
        assert!(matches!(
            effective_routes(
                &vec!["10.0.0.0/8".to_owned(); MAX_INPUT_NETWORKS + 1],
                net("192.168.133.0/24"),
                &[]
            ),
            Err(RouteBypassError::TooManyInputs {
                kind: "allowed routes",
                ..
            })
        ));
        assert!(matches!(
            effective_routes(
                &[],
                net("192.168.133.0/24"),
                &vec![net("10.0.0.0/8"); MAX_INPUT_NETWORKS + 1]
            ),
            Err(RouteBypassError::TooManyInputs {
                kind: "exclusions",
                ..
            })
        ));
    }

    #[test]
    fn subtraction_matches_address_membership_across_overlaps_and_boundaries() {
        let allowed = ["10.0.0.0/24", "10.0.1.0/25", "10.0.0.64/26"];
        for excluded in [
            vec![],
            vec!["10.0.0.0/32", "10.0.1.127/32"],
            vec!["10.0.0.0/25", "10.0.0.64/26", "10.0.1.64/26"],
            vec!["10.0.0.0/23"],
            vec!["9.0.0.0/8", "10.0.0.128/25", "11.0.0.0/8"],
        ] {
            let routes = plan(&allowed, &excluded);
            let allowed: Vec<_> = allowed.iter().map(|value| net(value)).collect();
            let excluded: Vec<_> = excluded.iter().map(|value| net(value)).collect();
            for suffix in 0..768 {
                let address = Ipv4Addr::from(0x0a00_0000 + suffix);
                let expected = allowed.iter().any(|route| route.contains(&address))
                    && !excluded.iter().any(|route| route.contains(&address));
                assert_eq!(
                    routes.iter().any(|route| route.contains(&address)),
                    expected,
                    "address {address}, exclusions {excluded:?}"
                );
            }
        }
    }
}
