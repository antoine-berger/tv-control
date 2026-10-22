use crate::{
    config::{Config, Server},
    process::{Invocation, Process},
};
use std::{collections::BTreeMap, sync::Arc};
use tv_application::Context;
use tv_domain::*;

pub struct HttpRequest {
    pub server: String,
    pub method: &'static str,
    pub path: String,
    pub mutating: bool,
}
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}
pub trait Http: Send + Sync {
    fn send(&self, request: HttpRequest, ctx: &Context<'_>) -> PortResult<HttpResponse>;
}
pub struct CurlHttp {
    pub config: Arc<Config>,
    pub process: Arc<dyn Process>,
}
impl Http for CurlHttp {
    fn send(&self, request: HttpRequest, ctx: &Context<'_>) -> PortResult<HttpResponse> {
        ctx.check()?;
        let server = self
            .config
            .servers
            .get(&request.server)
            .ok_or(Failure::before("unknown_server"))?;
        if !request.path.starts_with('/')
            || request.path.starts_with("//")
            || request.path.contains(['\r', '\n', '"', '\\', '#'])
        {
            return Err(Failure::before("invalid_http_path"));
        }
        let token = std::env::var(&server.token_env)
            .map_err(|_| Failure::before("missing_secret_reference"))?;
        let input = curl_config(
            server,
            request.method,
            &request.path,
            &token,
            ctx.remaining_ms(),
        )?;
        let output = self.process.run(
            Invocation {
                program: self.config.executables.curl.clone(),
                args: ["--disable", "--config", "-"]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                env: BTreeMap::new(),
                stdin: input.into_bytes(),
                mutating: request.mutating,
            },
            ctx,
        )?;
        let err = || {
            if request.mutating {
                Failure::uncertain("http_transport_failed")
            } else {
                Failure::before("http_transport_failed")
            }
        };
        if !output.success {
            return Err(err());
        }
        let split = output
            .stdout
            .iter()
            .rposition(|b| *b == b'\n')
            .ok_or_else(err)?;
        let status = std::str::from_utf8(&output.stdout[split + 1..])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(err)?;
        Ok(HttpResponse {
            status,
            body: output.stdout[..split].to_vec(),
        })
    }
}
pub fn curl_config(
    server: &Server,
    method: &str,
    path: &str,
    token: &str,
    remaining_ms: u64,
) -> PortResult<String> {
    if token.is_empty()
        || token.len() > 4096
        || !token
            .bytes()
            .all(|b| (33..=126).contains(&b) && !b"\"\\".contains(&b))
    {
        return Err(Failure::before("invalid_secret_value"));
    }
    if !matches!(method, "GET" | "POST") {
        return Err(Failure::before("unsupported_http_method"));
    }
    let seconds = remaining_ms as f64 / 1000.0;
    Ok(format!("url = \"{}{path}\"\nrequest = \"{method}\"\nheader = \"X-Emby-Token: {token}\"\nheader = \"Accept: application/json\"\nsilent\nshow-error\ngloboff\nproto = \"=http,https\"\nnoproxy = \"*\"\nmax-time = {seconds:.3}\nconnect-timeout = {seconds:.3}\nwrite-out = \"\\n%{{http_code}}\"\n", server.base_url))
}
pub fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
