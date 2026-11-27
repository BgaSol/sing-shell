use std::net::IpAddr;
pub fn network(value: &str) -> Result<(IpAddr, u8), String> {
    let (address, prefix) = value
        .trim()
        .split_once('/')
        .map(|(a, p)| (a, Some(p)))
        .unwrap_or((value.trim(), None));
    let address = address.parse::<IpAddr>().map_err(|_| "IP 地址格式无效")?;
    let max = if address.is_ipv4() { 32 } else { 128 };
    let prefix = prefix
        .map(|p| p.parse::<u8>().map_err(|_| "网段前缀无效"))
        .transpose()?
        .unwrap_or(max);
    if prefix > max {
        return Err("网段前缀超出地址长度".into());
    }
    let masked = match address {
        IpAddr::V4(a) => IpAddr::V4(
            (u32::from(a)
                & if prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - prefix)
                })
            .into(),
        ),
        IpAddr::V6(a) => IpAddr::V6(
            (u128::from(a)
                & if prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - prefix)
                })
            .into(),
        ),
    };
    Ok((masked, prefix))
}
pub fn overlap(a: (IpAddr, u8), b: (IpAddr, u8)) -> bool {
    if a.0.is_ipv4() != b.0.is_ipv4() {
        return false;
    }
    let bits = a.1.min(b.1);
    match (a.0, b.0) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            bits == 0 || (u32::from(a) >> (32 - bits)) == (u32::from(b) >> (32 - bits))
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            bits == 0 || (u128::from(a) >> (128 - bits)) == (u128::from(b) >> (128 - bits))
        }
        _ => false,
    }
}
