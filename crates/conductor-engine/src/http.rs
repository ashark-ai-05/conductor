//! Outbound HTTP (GitHub, OTLP collectors) that works where developers work: behind a proxy
//! (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, honouring `NO_PROXY`) and with a private CA
//! (the platform's certificate store, which `SSL_CERT_FILE` extends).

use std::net::Ipv4Addr;
use std::time::Duration;

/// An agent for `url`, through the proxy the environment names for it, if any.
pub fn agent_for(url: &str, timeout: Duration) -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new().timeout(timeout);
    if let Some(p) = proxy_for(url, |k| std::env::var(k).ok())
        && let Ok(p) = ureq::Proxy::new(&p)
    {
        b = b.proxy(p);
    }
    b.build()
}

fn host_of(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    Some((scheme, host))
}

fn in_cidr(host: &str, cidr: &str) -> bool {
    let (Some((net, bits)), Ok(ip)) = (cidr.split_once('/'), host.parse::<Ipv4Addr>()) else {
        return false;
    };
    let (Ok(net), Ok(bits)) = (net.parse::<Ipv4Addr>(), bits.parse::<u32>()) else {
        return false;
    };
    if bits > 32 {
        return false;
    }
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    u32::from(ip) & mask == u32::from(net) & mask
}

/// Whether `NO_PROXY` exempts `host`: `*`, exact names, domain suffixes (`.example.com`,
/// `*.example.com` or `example.com`), and IPv4 ranges (`10.0.0.0/8`).
fn exempt(host: &str, no_proxy: &str) -> bool {
    let host = host.to_ascii_lowercase();
    no_proxy
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .any(|e| {
            if e == "*" || e == host {
                return true;
            }
            if e.contains('/') {
                return in_cidr(&host, &e);
            }
            let suffix = e.trim_start_matches('*').trim_start_matches('.');
            host.ends_with(&format!(".{suffix}"))
        })
}

/// The proxy URL for `url`, if the environment names one and doesn't exempt the host.
pub fn proxy_for(url: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let (scheme, host) = host_of(url)?;
    let var = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| get(n).filter(|v| !v.trim().is_empty()))
    };
    if let Some(np) = var(&["NO_PROXY", "no_proxy"])
        && exempt(host, &np)
    {
        return None;
    }
    let proxy = match scheme {
        "https" => var(&["HTTPS_PROXY", "https_proxy"]),
        _ => var(&["HTTP_PROXY", "http_proxy"]),
    };
    proxy.or_else(|| var(&["ALL_PROXY", "all_proxy"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn https_goes_through_the_https_proxy() {
        let e = env(&[("HTTPS_PROXY", "http://proxy:3128")]);
        assert_eq!(
            proxy_for("https://api.github.com/repos/o/r/pulls", &e).as_deref(),
            Some("http://proxy:3128")
        );
        assert_eq!(proxy_for("http://collector:4318/v1/traces", &e), None);
    }

    #[test]
    fn no_proxy_exempts_names_suffixes_and_ranges() {
        let e = env(&[
            ("https_proxy", "http://proxy:3128"),
            ("http_proxy", "http://proxy:3128"),
            (
                "no_proxy",
                "localhost,127.0.0.0/8,.corp.example,*.svc.local,ghe.io",
            ),
        ]);
        for exempted in [
            "http://localhost:4318",
            "http://127.0.0.1:4318",
            "https://git.corp.example/api",
            "http://otel.svc.local:4318",
            "https://api.ghe.io/v3",
            "https://ghe.io",
        ] {
            assert_eq!(proxy_for(exempted, &e), None, "{exempted}");
        }
        assert!(proxy_for("https://api.github.com", &e).is_some());
        assert!(proxy_for("http://10.1.2.3:4318", &e).is_some());
    }

    #[test]
    fn all_proxy_is_the_fallback_and_star_exempts_everything() {
        let e = env(&[("ALL_PROXY", "http://p:1")]);
        assert_eq!(proxy_for("https://a.b", &e).as_deref(), Some("http://p:1"));
        let e = env(&[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", "*")]);
        assert_eq!(proxy_for("https://a.b", &e), None);
        assert_eq!(proxy_for("not a url", env(&[("ALL_PROXY", "x")])), None);
    }
}
