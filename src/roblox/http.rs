use std::ffi::c_void;

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_ETAG, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::core::{PCWSTR, w};

use super::updater::HttpResult;
use crate::util::text::{from_wide, wide};

struct Internet(*mut c_void);
impl Drop for Internet {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

const CAP: usize = 1024 * 1024;

/// `https://host[:port]/path`; None for anything else.
fn split_url(url: &str) -> Option<(&str, u16, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (authority, INTERNET_DEFAULT_HTTPS_PORT),
    };
    (!host.is_empty()).then_some((host, port, if path.is_empty() { "/" } else { path }))
}

/// The response with `body` left empty, and the raw body (capped at `cap` + 1 byte so callers can detect oversize).
/// `headers` are extra request headers separated by CRLF; `timeout_ms` applies to each of resolve, connect, send and
/// receive. Redirects are followed (WinHTTP's default policy, which only refuses HTTPS to HTTP). `progress` gets the
/// bytes read so far and the Content-Length when the server sent one, after each read.
fn fetch(
    url: &str,
    headers: &str,
    cap: usize,
    timeout_ms: i32,
    progress: &mut dyn FnMut(usize, Option<usize>),
) -> (HttpResult, Vec<u8>) {
    let limit = cap + 1;
    let mut res = HttpResult::default();
    let mut body = Vec::new();
    let Some((host, port, path)) = split_url(url) else {
        return (res, Vec::new());
    };
    let (host, path) = (wide(host), wide(path));
    unsafe {
        let ses = Internet(WinHttpOpen(
            w!("curl/8"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if ses.0.is_null() {
            return (res, Vec::new());
        }
        let _ = WinHttpSetTimeouts(ses.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms);
        let con = Internet(WinHttpConnect(ses.0, PCWSTR(host.as_ptr()), port, 0));
        if con.0.is_null() {
            return (res, Vec::new());
        }
        let req = Internet(WinHttpOpenRequest(
            con.0,
            w!("GET"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return (res, Vec::new());
        }
        if !headers.is_empty() {
            let h: Vec<u16> = headers.encode_utf16().collect();
            let _ = WinHttpAddRequestHeaders(req.0, &h, WINHTTP_ADDREQ_FLAG_ADD);
        }
        if WinHttpSendRequest(req.0, None, None, 0, 0, 0).is_err()
            || WinHttpReceiveResponse(req.0, std::ptr::null_mut()).is_err()
        {
            return (res, Vec::new());
        }

        let (mut status, mut sz) = (0u32, 4u32);
        let _ = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut sz,
            std::ptr::null_mut(),
        );
        let mut et = [0u16; 256];
        sz = size_of_val(&et) as u32;
        if WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_ETAG,
            PCWSTR::null(),
            Some(et.as_mut_ptr().cast()),
            &mut sz,
            std::ptr::null_mut(),
        )
        .is_ok()
        {
            res.etag = from_wide(&et[..(sz as usize / 2).min(et.len())]);
        }

        let (mut len, mut sz) = (0u32, 4u32);
        let total = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut len).cast()),
            &mut sz,
            std::ptr::null_mut(),
        )
        .is_ok()
        .then_some(len as usize);

        let mut buf = [0u8; 8192];
        while body.len() < limit {
            let mut got = 0u32;
            if WinHttpReadData(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut got).is_err() {
                return (res, Vec::new()); // cut off mid-body: status 0, so a partial file is never kept
            }
            if got == 0 {
                break;
            }
            body.extend_from_slice(&buf[..(got as usize).min(limit - body.len())]);
            progress(body.len(), total);
        }
        res.status = status as i32;
    }
    (res, body)
}

/// HTTPS only; body capped at 1 MB + 1 byte so callers can detect oversize. Status 0 on any failure, including a
/// connection lost mid-body.
pub fn win_http_get(url: &str, if_none_match: &str) -> HttpResult {
    let h = if if_none_match.is_empty() {
        String::new()
    } else {
        format!("If-None-Match: {if_none_match}")
    };
    let (mut res, body) = fetch(url, &h, CAP, 10000, &mut |_, _| {});
    res.body = String::from_utf8_lossy_owned(body);
    res
}

/// `win_http_get` with the body kept as bytes, for downloads that are not text: (status, body).
pub fn win_http_get_bytes(url: &str) -> (i32, Vec<u8>) {
    let (res, body) = fetch(url, "", CAP, 10000, &mut |_, _| {});
    (res.status, body)
}

/// `win_http_get_bytes` with extra headers, a body cap of `cap` + 1 byte, per-step timeouts and download progress
/// (bytes so far, Content-Length if known): (status, body).
pub(crate) fn win_http_get_capped(
    url: &str,
    headers: &str,
    cap: usize,
    timeout_ms: i32,
    progress: &mut dyn FnMut(usize, Option<usize>),
) -> (i32, Vec<u8>) {
    let (res, body) = fetch(url, headers, cap, timeout_ms, progress);
    (res.status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_split_url() {
        assert_eq!(split_url("https://a.b/x/y.json"), Some(("a.b", 443, "/x/y.json")));
        assert_eq!(split_url("https://a.b:8443"), Some(("a.b", 8443, "/")));
        assert_eq!(split_url("http://a.b/x"), None);
        assert_eq!(split_url("https://:1/x"), None);
        assert_eq!(win_http_get("http://example.com/", "").status, 0); // HTTPS only: no request is made
    }
}
