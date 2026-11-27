use std::net::IpAddr;

fn range(destination: &str, v6: bool) -> Option<(u128, u128)> {
    let bits = if v6 { 128 } else { 32 };
    let (address, prefix) = if destination == "default" {
        ("", 0)
    } else {
        let (address, prefix) = destination
            .split_once('/')
            .map(|(a, p)| Some((a, p.parse::<u32>().ok()?)))
            .unwrap_or_else(|| {
                Some((
                    destination,
                    if v6 {
                        128
                    } else {
                        destination.split('.').count() as u32 * 8
                    },
                ))
            })?;
        (address, prefix)
    };
    if prefix > bits {
        return None;
    }
    let start = if address.is_empty() {
        0
    } else if v6 {
        let address = address.split('%').next()?;
        match address.parse::<IpAddr>().ok()? {
            IpAddr::V6(value) => u128::from(value),
            _ => return None,
        }
    } else {
        let mut octets = [0u8; 4];
        for (index, value) in address.split('.').enumerate() {
            *octets.get_mut(index)? = value.parse().ok()?;
        }
        u32::from_be_bytes(octets) as u128
    };
    let host = bits - prefix;
    let mask = if host == 128 {
        u128::MAX
    } else {
        (1u128 << host) - 1
    };
    Some((start & !mask, start | mask))
}

pub(super) fn conflicts_with_fakeip(destination: &str) -> bool {
    range(destination, false).is_some_and(|(first, last)| {
        // Default and half-default routes express internet access, not a reserved local network.
        last - first < (1u128 << 31) - 1 && first <= 0xc613ffff && last >= 0xc6120000
    })
}

pub(super) fn covered<'a>(destinations: impl IntoIterator<Item = &'a str>, v6: bool) -> bool {
    let mut ranges: Vec<_> = destinations
        .into_iter()
        .filter_map(|value| range(value, v6))
        .collect();
    ranges.sort_unstable();
    let covers = |mut next: u128, last: u128| {
        for &(start, end) in &ranges {
            if end < next {
                continue;
            }
            if start > next {
                return false;
            }
            if end >= last {
                return true;
            }
            next = end + 1;
        }
        false
    };
    // Official Darwin auto routes exclude 0/8, loopback and ::/8.
    if v6 {
        covers(1u128 << 120, u128::MAX)
    } else {
        covers(0x01000000, 0x7effffff) && covers(0x80000000, 0xffffffff)
    }
}

pub(super) fn matches<'a>(
    destinations: impl IntoIterator<Item = &'a str>,
    excluded: &[String],
    v6: bool,
) -> bool {
    let destinations: Vec<_> = destinations.into_iter().collect();
    let excluded: Vec<_> = excluded.iter().filter(|p| p.contains(':') == v6).collect();
    let ranges: Vec<_> = destinations.iter().filter_map(|p| range(p, v6)).collect();
    for prefix in &excluded {
        let Some((first, last)) = range(prefix, v6) else {
            return false;
        };
        if ranges.iter().any(|&(a, b)| a <= last && b >= first) {
            return false;
        }
    }
    covered(
        destinations
            .into_iter()
            .chain(excluded.into_iter().map(String::as_str)),
        v6,
    )
}

#[cfg(any(test, target_os = "macos"))]
pub(super) fn macos(text: &str, interface: &str, v6: bool) -> bool {
    macos_expected(text, interface, v6, &[])
}
#[cfg(any(test, target_os = "macos"))]
pub(super) fn macos_expected(text: &str, interface: &str, v6: bool, excluded: &[String]) -> bool {
    matches(
        text.lines().filter_map(|line| {
            let mut parts = line.split_whitespace();
            let destination = parts.next()?;
            parts.any(|part| part == interface).then_some(destination)
        }),
        excluded,
        v6,
    )
}

#[cfg(any(test, target_os = "linux"))]
pub(super) fn linux(routes: &str, rules: &str, interface: &str, v6: bool) -> Result<bool, String> {
    linux_expected(routes, rules, interface, v6, &[])
}
#[cfg(any(test, target_os = "linux"))]
pub(super) fn linux_expected(
    routes: &str,
    rules: &str,
    interface: &str,
    v6: bool,
    excluded: &[String],
) -> Result<bool, String> {
    let routes: Vec<serde_json::Value> =
        serde_json::from_str(routes).map_err(|_| "系统路由输出无效")?;
    let rules: Vec<serde_json::Value> =
        serde_json::from_str(rules).map_err(|_| "系统策略路由输出无效")?;
    let table = |value: &serde_json::Value| match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Number(value) => value.to_string(),
        _ => "main".into(),
    };
    let tables: std::collections::BTreeSet<_> = routes
        .iter()
        .filter(|r| r["dev"] == interface)
        .map(|r| table(&r["table"]))
        .collect();
    Ok(tables.into_iter().any(|t| {
        let active = rules
            .iter()
            .any(|rule| table(&rule["table"]) == t && rule.get("table").is_some());
        active
            && matches(
                routes
                    .iter()
                    .filter(|r| r["dev"] == interface && table(&r["table"]) == t)
                    .filter_map(|r| r["dst"].as_str()),
                excluded,
                v6,
            )
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn exclusions_require_both_actual_holes_and_complete_remaining_coverage() {
        let excluded = vec!["128.0.0.0/2".to_string()];
        assert!(matches(["0.0.0.0/1", "192.0.0.0/2"], &excluded, false));
        assert!(!matches(["0.0.0.0/0"], &excluded, false));
        assert!(!matches(["0.0.0.0/2", "192.0.0.0/2"], &excluded, false));
        assert!(matches(["::/1", "c000::/2"], &["8000::/2".into()], true));
        assert!(!matches(["::/0"], &["8000::/2".into()], true));
    }
    #[test]
    fn fakeip_conflicts_exclude_default_routes_but_include_real_pool_routes() {
        assert!(super::conflicts_with_fakeip("198.18.0.0/15"));
        assert!(super::conflicts_with_fakeip("198.19.3.4/32"));
        assert!(super::conflicts_with_fakeip("198.0.0.0/8"));
        for value in ["default", "0.0.0.0/0", "128.0.0.0/1", "203.0.113.0/24"] {
            assert!(!super::conflicts_with_fakeip(value));
        }
    }
    use super::*;
    #[test]
    fn partial_routes_never_imply_default_coverage() {
        assert!(!covered(["128.0/1"], false));
        assert!(!covered(["172.19.0.0/30"], false));
        assert!(covered(["0.0.0.0/1", "128.0.0.0/1"], false));
        let native = "1 gw flags utun223\n2/7 gw flags utun223\n4/6 gw flags utun223\n8/5 gw flags utun223\n16/4 gw flags utun223\n32/3 gw flags utun223\n64/2 gw flags utun223\n128.0/1 gw flags utun223";
        assert!(macos(native, "utun223", false));
        assert!(!macos(&native.replace("16/4", "16/5"), "utun223", false));
        assert!(covered(
            [
                "100::/8", "200::/7", "400::/6", "800::/5", "1000::/4", "2000::/3", "4000::/2",
                "8000::/1"
            ],
            true
        ));
        assert!(!covered(["8000::/1"], true));
    }
    #[test]
    fn linux_requires_policy_selection_and_complete_routes_in_one_table() {
        let routes = r#"[{"dev":"sing-shell","dst":"default","table":2022}]"#;
        assert!(!linux(routes, "[]", "sing-shell", false).unwrap());
        assert!(linux(
            routes,
            r#"[{"priority":9000,"table":2022}]"#,
            "sing-shell",
            false
        )
        .unwrap());
        assert!(!linux(
            &routes.replace("default", "172.19.0.0/30"),
            r#"[{"table":2022}]"#,
            "sing-shell",
            false
        )
        .unwrap());
        assert!(linux("invalid", "[]", "sing-shell", false).is_err());
    }
}
