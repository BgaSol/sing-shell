use serde_json::Value;
use std::{
    net::{IpAddr, Ipv4Addr, TcpListener},
    time::Duration,
};

pub struct Endpoint {
    pub port: u16,
    pub secret: String,
}
impl Endpoint {
    pub fn new() -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        Ok(Self {
            port: listener.local_addr().map_err(|e| e.to_string())?.port(),
            secret: crate::platform::authorization::nonce()?,
        })
    }
    pub fn config(&self) -> Value {
        serde_json::json!({"external_controller":format!("127.0.0.1:{}",self.port),"secret":self.secret})
    }
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
    pub async fn query(
        &self,
        client: &reqwest::Client,
        name: &str,
        kind: &str,
    ) -> Result<Vec<IpAddr>, String> {
        let mut url =
            reqwest::Url::parse(&self.url("/dns/query")).map_err(|_| "核心控制地址无效")?;
        url.query_pairs_mut()
            .append_pair("name", name)
            .append_pair("type", kind);
        let mut response = client
            .get(url)
            .bearer_auth(&self.secret)
            .send()
            .await
            .map_err(|_| "绕过域名解析接口不可用")?
            .error_for_status()
            .map_err(|_| "绕过域名解析接口鉴权或状态异常")?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "绕过域名解析响应中断")?
        {
            if bytes.len() + chunk.len() > 64 * 1024 {
                return Err("绕过域名解析响应超限".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        answers(&bytes, name, kind)
    }
}
pub fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|_| "核心控制客户端无法初始化".into())
}
fn answers(bytes: &[u8], name: &str, kind: &str) -> Result<Vec<IpAddr>, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "绕过域名解析响应格式无效")?;
    let expected = match kind {
        "A" => 1,
        "AAAA" => 28,
        _ => return Err("不支持的地址查询类型".into()),
    };
    if value["Status"] != 0 || value["TC"] != false {
        return Err("绕过域名解析失败或响应被截断".into());
    }
    let question = value["Question"]
        .as_array()
        .filter(|q| q.len() == 1)
        .ok_or("绕过域名解析问题无效")?;
    if question[0]["Qtype"] != expected
        || !question[0]["Name"]
            .as_str()
            .is_some_and(|n| n.trim_end_matches('.').eq_ignore_ascii_case(name))
    {
        return Err("绕过域名解析问题不符".into());
    }
    let mut addresses = Vec::new();
    if let Some(items) = value.get("Answer") {
        for item in items.as_array().ok_or("绕过域名解析记录无效")? {
            if item["type"] == expected {
                let ip: IpAddr = item["data"]
                    .as_str()
                    .ok_or("绕过域名 IP 无效")?
                    .parse()
                    .map_err(|_| "绕过域名 IP 无效")?;
                if ip.is_ipv4() != (expected == 1) {
                    return Err("绕过域名地址族不符".into());
                }
                addresses.push(ip);
            }
        }
    }
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_question_status_family_and_truncation() {
        let good = serde_json::json!({"Status":0,"TC":false,"Question":[{"Name":"id.test.","Qtype":1}],"Answer":[{"type":1,"data":"192.0.2.1"}]});
        assert_eq!(
            answers(&serde_json::to_vec(&good).unwrap(), "id.test", "A")
                .unwrap()
                .len(),
            1
        );
        for changed in [
            serde_json::json!({"Status":2}),
            {
                let mut v = good.clone();
                v["TC"] = true.into();
                v
            },
            {
                let mut v = good.clone();
                v["Question"][0]["Name"] = "other.test.".into();
                v
            },
        ] {
            assert!(answers(&serde_json::to_vec(&changed).unwrap(), "id.test", "A").is_err())
        }
    }
}
