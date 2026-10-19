//! Smoke test: runs real uploads against an in-process HTTP server and prints
//! the AWS SigV4 documentation vectors. Output goes to
//! `target/smoke/snap-upload/`.

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use snap_core::{
    secrets::{MemoryStore, SecretStore},
    settings::{CustomUploader, Destination, DestinationKind, LinkFormat, Shortener},
};
use snap_upload::{
    UploadRequest, format_link, oauth, presets, server, shorten,
    sigv4::{self, Credentials},
    sxcu, uploader_for,
};

struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

type Reply = (u16, Vec<(&'static str, String)>, Vec<u8>);

fn read_request(stream: &mut TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Req { method, path, headers, body })
}

/// Pull `name="..."` fields (and the file bytes) out of a multipart body.
type Multipart = (Vec<(String, String)>, Option<(String, Vec<u8>)>);

fn parse_multipart(req: &Req) -> Multipart {
    let ct = req.header("content-type").unwrap_or_default();
    let Some(boundary) = ct.split("boundary=").nth(1) else { return (vec![], None) };
    let delim = format!("--{boundary}");
    let text = String::from_utf8_lossy(&req.body).into_owned();
    let (mut fields, mut file) = (vec![], None);
    for part in text.split(&delim).skip(1) {
        let Some((head, rest)) = part.split_once("\r\n\r\n") else { continue };
        let content = rest.strip_suffix("\r\n").unwrap_or(rest);
        let name = head.split("name=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or_default().to_string();
        if let Some(filename) = head.split("filename=\"").nth(1).and_then(|s| s.split('"').next()) {
            file = Some((filename.to_string(), content.as_bytes().to_vec()));
        } else {
            fields.push((name, content.to_string()));
        }
    }
    (fields, file)
}

fn json(code: u16, v: serde_json::Value) -> Reply {
    (code, vec![("Content-Type", "application/json".into())], v.to_string().into_bytes())
}

fn handle(req: Req, base: &str, dav: &Mutex<HashSet<String>>) -> Reply {
    println!("  [server] {} {}", req.method, req.path);
    let secret = "SnapSecretKey/ForTests+1234567890abcdefghij";
    let route = req.path.split('?').next().unwrap_or_default().to_string();
    match (req.method.as_str(), route.as_str()) {
        ("POST", "/mp") => {
            let (fields, file) = parse_multipart(&req);
            let (name, data) = file.unwrap_or_default();
            json(
                200,
                serde_json::json!({
                    "success": true,
                    "data": { "link": format!("{base}/f/{name}"), "items": [{ "u": "first" }, { "u": "second" }], "size": data.len() },
                    "fields": fields.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
                }),
            )
        }
        ("POST", "/xml") => (
            200,
            vec![("Content-Type", "application/xml".into())],
            b"<resp><files><file id=\"1\"><url>u1</url></file><file id=\"2\"><url>http://x/two.png</url></file></files></resp>".to_vec(),
        ),
        ("POST", "/text") => (200, vec![], b"saved id=4711 done".to_vec()),
        ("POST", "/hdr") => (200, vec![("X-Delete", format!("{base}/del/123"))], b"ok".to_vec()),
        ("POST", "/jsonbody") => {
            json(200, serde_json::json!({ "echo": serde_json::from_slice::<serde_json::Value>(&req.body).unwrap_or_default() }))
        }
        ("POST", "/fail") => json(500, serde_json::json!({ "error": "disk is full" })),
        ("POST", "/api/v1/upload") => {
            if req.header("authorization") != Some("Bearer team-token") {
                return json(401, serde_json::json!({ "error": "bad token" }));
            }
            let (fields, file) = parse_multipart(&req);
            let (name, _) = file.unwrap_or_default();
            println!("  [server] team fields: {fields:?}");
            json(
                200,
                serde_json::json!({ "id": "abc123", "url": format!("{base}/abc123"), "raw_url": format!("{base}/abc123/raw/{name}"), "delete_url": "/api/v1/items/abc123" }),
            )
        }
        ("POST", "/api/v1/shorten") => json(200, serde_json::json!({ "short_url": format!("{base}/s/xYz") })),
        ("GET", "/api/v1/me") => json(200, serde_json::json!({ "id": "u1", "name": "Napoleon", "workspace": "Elba" })),
        ("GET", p) if p.starts_with("/api/v1/items") => {
            json(200, serde_json::json!({ "items": [{ "id": "abc123", "url": format!("{base}/abc123"), "views": 3, "created_ms": 1 }] }))
        }
        ("GET", p) if p.starts_with("/api/v1/events") => json(
            200,
            serde_json::json!([{ "id": "e1", "kind": "view", "item_id": "abc123", "at_ms": 5 }, { "id": "e2", "kind": "weird", "item_id": "abc123", "at_ms": 6 }]),
        ),
        ("DELETE", p) if p.starts_with("/api/v1/items/") => (204, vec![], vec![]),
        ("MKCOL", p) => {
            dav.lock().unwrap().insert(p.trim_end_matches('/').to_string());
            (201, vec![], vec![])
        }
        ("PUT", p) if p.starts_with("/dav/") => {
            let parent = p.rsplit_once('/').map(|(a, _)| a).unwrap_or("");
            let known = dav.lock().unwrap().contains(parent) || parent == "/dav" && dav.lock().unwrap().contains("/dav");
            if known { (201, vec![], vec![]) } else { (409, vec![], vec![]) }
        }
        ("PUT", p) if p.starts_with("/snaps/") || req.header("host").is_some_and(|h| h.starts_with("snaps.")) => verify_sigv4(&req, secret),
        _ => (404, vec![], b"not found".to_vec()),
    }
}

/// Recompute the signature the way a real S3 would, from what arrived on the wire.
fn verify_sigv4(req: &Req, secret: &str) -> Reply {
    let auth = req.header("authorization").unwrap_or_default();
    let field = |name: &str| auth.split(name).nth(1).and_then(|s| s.split([',', ' ']).next()).unwrap_or_default().to_string();
    let credential = field("Credential=");
    let signed_headers = field("SignedHeaders=");
    let signature = field("Signature=");
    let parts: Vec<&str> = credential.split('/').collect();
    let headers: Vec<(String, String)> =
        signed_headers.split(';').map(|n| (n.to_string(), req.header(n).unwrap_or_default().to_string())).collect();
    let payload_hash = req.header("x-amz-content-sha256").unwrap_or_default();
    let body_ok = sigv4::sha256_hex(&req.body) == payload_hash;
    let creds = Credentials { access_key: parts[0], secret_key: secret };
    let expected =
        sigv4::sign("PUT", &req.path, "", &headers, payload_hash, &creds, parts[2], "s3", req.header("x-amz-date").unwrap_or_default());
    let ok = body_ok && expected.signature == signature;
    println!(
        "  [server] SigV4 {} (signed headers: {signed_headers}; acl={:?}; content-type={:?})",
        if ok { "VALID" } else { "INVALID" },
        req.header("x-amz-acl"),
        req.header("content-type")
    );
    if ok {
        (200, vec![("ETag", "\"abc\"".into())], vec![])
    } else {
        (403, vec![], b"<Error><Message>SignatureDoesNotMatch</Message></Error>".to_vec())
    }
}

fn spawn_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dav = Arc::new(Mutex::new(HashSet::new()));
    let base2 = base.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (base, dav) = (base2.clone(), dav.clone());
            thread::spawn(move || {
                let mut stream = stream;
                let Some(req) = read_request(&mut stream) else { return };
                let (code, headers, body) = handle(req, &base, &dav);
                let mut head = format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                for (k, v) in headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            });
        }
    });
    base
}

/// Minimal passive-mode FTP server rooted at `root`, enough for one STOR.
fn spawn_ftp(root: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for control in listener.incoming().flatten() {
            let root = root.clone();
            thread::spawn(move || ftp_session(control, root));
        }
    });
    port
}

fn ftp_session(mut control: TcpStream, root: PathBuf) {
    let reader = BufReader::new(control.try_clone().unwrap());
    let reply = |c: &mut TcpStream, s: &str| {
        println!("  [ftp] -> {s}");
        let _ = write!(c, "{s}\r\n");
    };
    reply(&mut control, "220 ready");
    let mut cwd = root.clone();
    let mut data: Option<TcpListener> = None;
    for line in reader.lines().map_while(std::result::Result::ok) {
        println!("  [ftp] <- {line}");
        let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
        match cmd.to_ascii_uppercase().as_str() {
            "USER" => reply(&mut control, "331 password please"),
            "PASS" => reply(&mut control, "230 welcome"),
            "SYST" => reply(&mut control, "215 UNIX Type: L8"),
            "TYPE" => reply(&mut control, "200 ok"),
            "CWD" => {
                let target = if arg.starts_with('/') { root.join(arg.trim_start_matches('/')) } else { cwd.join(arg) };
                if target.is_dir() {
                    cwd = target;
                    reply(&mut control, "250 ok");
                } else {
                    reply(&mut control, "550 no such directory");
                }
            }
            "MKD" => {
                std::fs::create_dir_all(cwd.join(arg)).unwrap();
                reply(&mut control, &format!("257 \"{arg}\" created"));
            }
            "PASV" => {
                let l = TcpListener::bind("127.0.0.1:0").unwrap();
                let p = l.local_addr().unwrap().port();
                data = Some(l);
                reply(&mut control, &format!("227 Entering Passive Mode (127,0,0,1,{},{})", p >> 8, p & 255));
            }
            "STOR" => {
                let Some(l) = data.take() else { return reply(&mut control, "425 no data connection") };
                reply(&mut control, "150 send it");
                let (mut s, _) = l.accept().unwrap();
                let mut bytes = Vec::new();
                s.read_to_end(&mut bytes).unwrap();
                std::fs::write(cwd.join(arg), bytes).unwrap();
                reply(&mut control, "226 stored");
            }
            "QUIT" => return reply(&mut control, "221 bye"),
            _ => reply(&mut control, "502 not implemented"),
        }
    }
}

fn dest(id: &str, kind: DestinationKind, secret: Option<&str>) -> Destination {
    Destination { id: id.into(), name: id.into(), kind, secret_ref: secret.map(String::from) }
}

fn custom(url: String, body: &str, f: impl FnOnce(&mut CustomUploader)) -> DestinationKind {
    let mut c = CustomUploader {
        request_method: "POST".into(),
        request_url: url,
        body: body.into(),
        file_form_name: "file".into(),
        ..Default::default()
    };
    f(&mut c);
    DestinationKind::CustomHttp(c)
}

fn main() {
    let out = PathBuf::from("C:/Users/3mpti/snapoleon/target/smoke/snap-upload");
    std::fs::create_dir_all(&out).unwrap();
    let report = std::cell::RefCell::new(String::new());
    macro_rules! say {
        ($($a:tt)*) => {{
            let l = format!($($a)*);
            println!("{l}");
            report.borrow_mut().push_str(&l);
            report.borrow_mut().push('\n');
        }};
    }

    say!("== SigV4: AWS documentation vectors ==");
    let creds = Credentials { access_key: "AKIAIOSFODNN7EXAMPLE", secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY" };
    let empty = sigv4::sha256_hex(b"");
    let get = sigv4::sign(
        "GET",
        &sigv4::canonical_uri("/test.txt"),
        "",
        &[
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("range".into(), "bytes=0-9".into()),
            ("x-amz-content-sha256".into(), empty),
            ("x-amz-date".into(), "20130524T000000Z".into()),
        ],
        &sigv4::sha256_hex(b""),
        &creds,
        "us-east-1",
        "s3",
        "20130524T000000Z",
    );
    let get_expected = "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41";
    say!(
        "GET Object   signature {}  expected {get_expected}  -> {}",
        get.signature,
        if get.signature == get_expected { "MATCH" } else { "MISMATCH" }
    );
    say!("             {}", get.authorization);

    let body = b"Welcome to Amazon S3.";
    let put_hash = sigv4::sha256_hex(body);
    let put = sigv4::sign(
        "PUT",
        &sigv4::canonical_uri("/test$file.text"),
        "",
        &[
            ("date".into(), "Fri, 24 May 2013 00:00:00 GMT".into()),
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("x-amz-content-sha256".into(), put_hash.clone()),
            ("x-amz-date".into(), "20130524T000000Z".into()),
            ("x-amz-storage-class".into(), "REDUCED_REDUNDANCY".into()),
        ],
        &put_hash,
        &creds,
        "us-east-1",
        "s3",
        "20130524T000000Z",
    );
    let put_expected = "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd";
    say!(
        "PUT Object   signature {}  expected {put_expected}  -> {}",
        put.signature,
        if put.signature == put_expected { "MATCH" } else { "MISMATCH" }
    );

    let base = spawn_server();
    say!("\n== local server at {base} ==");
    let secrets = MemoryStore::default();
    let png: Vec<u8> = (0..70_000u32).map(|i| 32 + (i * 31 % 90) as u8).collect();
    let req = UploadRequest {
        data: &png,
        file_name: "My shot (1) ü.png",
        mime: "image/png",
        title: Some("Demo \"title\""),
        expiry_hours: Some(24),
        password: Some("pw"),
    };
    let last = std::cell::Cell::new((0u64, 0u64));
    let ticks = std::cell::Cell::new(0u32);
    let progress = |s: u64, t: u64| {
        last.set((s, t));
        ticks.set(ticks.get() + 1);
    };

    let run = |label: &str, d: Destination| {
        let up = match uploader_for(&d, &secrets) {
            Ok(u) => u,
            Err(e) => return say!("{label}: build failed: {e}"),
        };
        ticks.set(0);
        match up.upload(&req, &progress) {
            Ok(r) => say!("{label}: OK  {r:?}  [progress {:?}, {} ticks]", last.get(), ticks.get()),
            Err(e) => say!("{label}: ERR {e}"),
        }
    };

    say!("\n== CustomHttp ==");
    run(
        "multipart + json path",
        dest(
            "c1",
            custom(format!("{base}/mp"), "multipart_form_data", |c| {
                c.arguments = vec![("title".into(), "{filename}".into()), ("tag".into(), "{random:a|b|c}".into())];
                c.parameters = vec![("v".into(), "1".into())];
                c.headers = vec![("X-Test".into(), "yes".into())];
                c.url = "{json:data.link}".into();
                c.thumbnail_url = "{json:data.items[1].u}".into();
                c.deletion_url = "{base64:abc}-{json:data.size}".into();
            }),
            None,
        ),
    );
    run(
        "xml xpath",
        dest("c2", custom(format!("{base}/xml"), "multipart_form_data", |c| c.url = "{xml://file[@id='2']/url}".into()), None),
    );
    run("regex group", dest("c3", custom(format!("{base}/text"), "binary", |c| c.url = "https://x/{regex:id=(\\d+)|1}".into()), None));
    run(
        "header + response",
        dest(
            "c4",
            custom(format!("{base}/hdr"), "binary", |c| {
                c.url = "{response}".into();
                c.deletion_url = "{header:x-delete}".into();
            }),
            None,
        ),
    );
    run(
        "json body",
        dest(
            "c5",
            custom(format!("{base}/jsonbody"), "json", |c| {
                c.arguments = vec![(String::new(), "{\"name\": \"{filename}\", \"title\": \"{title}\"}".into())];
                c.url = "{json:echo.name}".into();
            }),
            None,
        ),
    );
    run("error message", dest("c6", custom(format!("{base}/fail"), "binary", |c| c.error_message = "{json:error}".into()), None));

    say!("\n== LocalFolder ==");
    let folder = out.join("uploads");
    let _ = std::fs::remove_dir_all(&folder);
    let kind = DestinationKind::LocalFolder { dir: folder.display().to_string(), public_base_url: "https://cdn.example.com/shots".into() };
    run("local #1", dest("l", kind.clone(), None));
    run("local #2 (name clash)", dest("l", kind, None));
    let kind = DestinationKind::LocalFolder { dir: folder.display().to_string(), public_base_url: String::new() };
    run("local (file:// url)", dest("l2", kind, None));
    let mut names: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    say!("files on disk: {names:?}");

    say!("\n== S3 (path-style against the local server, SigV4 verified server-side) ==");
    secrets.set("s3", "SnapSecretKey/ForTests+1234567890abcdefghij").unwrap();
    run(
        "s3 random name",
        dest(
            "s3a",
            DestinationKind::S3 {
                endpoint: base.clone(),
                region: "ru-central1".into(),
                bucket: "snaps".into(),
                access_key_id: "AKIATESTKEY".into(),
                path_style: true,
                prefix: "shots/{yyyy}/".into(),
                public_base_url: "https://img.example.com".into(),
                random_code_len: 10,
                acl_public_read: true,
            },
            Some("s3"),
        ),
    );
    run(
        "s3 keep name",
        dest(
            "s3b",
            DestinationKind::S3 {
                endpoint: base.clone(),
                region: "us-east-1".into(),
                bucket: "snaps".into(),
                access_key_id: "AKIATESTKEY".into(),
                path_style: true,
                prefix: String::new(),
                public_base_url: String::new(),
                random_code_len: 0,
                acl_public_read: false,
            },
            Some("s3"),
        ),
    );
    let vhost = base.replace("127.0.0.1", "s3.localhost");
    run(
        "s3 virtual-hosted (snaps.s3.localhost)",
        dest(
            "s3c",
            DestinationKind::S3 {
                endpoint: vhost,
                region: "us-east-1".into(),
                bucket: "snaps".into(),
                access_key_id: "AKIATESTKEY".into(),
                path_style: false,
                prefix: "v/".into(),
                public_base_url: String::new(),
                random_code_len: 6,
                acl_public_read: false,
            },
            Some("s3"),
        ),
    );
    for p in presets::s3() {
        say!("preset {:<22} {:<48} {:<12} path_style={}", p.name, p.endpoint, p.region, p.path_style);
    }

    say!("\n== WebDAV (409 -> MKCOL parents -> PUT) ==");
    run(
        "webdav",
        dest("dav", DestinationKind::WebDav { url: format!("{base}/dav"), user: "u".into(), public_base_url: String::new() }, Some("s3")),
    );

    say!("\n== Team server client ==");
    secrets.set("team", "team-token").unwrap();
    run("snap server upload", dest("srv", DestinationKind::SnapServer { url: base.clone() }, Some("team")));
    let client = server::Client::new(&base, "team-token");
    say!("me: {:?}", client.me().unwrap());
    say!("list: {:?}", client.list(1).unwrap());
    say!("events: {:?}", client.events(0).unwrap());
    client.delete("abc123").unwrap();
    say!("delete: ok");
    say!("bad token: {:?}", server::Client::new(&base, "nope").upload(&req, &|_, _| {}).map(|_| ()).map_err(|e| e.to_string()));
    secrets.set(&snap_upload::snap_server_secret_key(&base), "team-token").unwrap();
    say!("shorten (team server): {}", shorten("https://example.com/long", &Shortener::SnapServer, Some(&base), &secrets).unwrap());

    say!("\n== FTP (passive mode against a local server) ==");
    let ftp_root = out.join("ftp_root");
    let _ = std::fs::remove_dir_all(&ftp_root);
    std::fs::create_dir_all(&ftp_root).unwrap();
    let ftp_port = spawn_ftp(ftp_root.clone());
    let ftp_kind = DestinationKind::Ftp {
        host: "127.0.0.1".into(),
        port: ftp_port,
        user: "me".into(),
        secure: false,
        dir: "/public/shots".into(),
        public_base_url: "https://files.example.com/shots".into(),
    };
    run("ftp", dest("ftp", ftp_kind, Some("s3")));
    let stored = std::fs::read(ftp_root.join("public/shots/My shot (1) ü.png")).map(|b| b.len());
    say!("file on the FTP server: {stored:?} bytes");
    secrets.set("rebex", "password").unwrap();
    let ftps = dest(
        "ftps",
        DestinationKind::Ftp {
            host: "test.rebex.net".into(),
            port: 21,
            user: "demo".into(),
            secure: true,
            dir: "/".into(),
            public_base_url: String::new(),
        },
        Some("rebex"),
    );
    say!(
        "FTPS test.rebex.net (live, explicit TLS via rustls): {:?}",
        uploader_for(&ftps, &secrets).and_then(|u| u.test()).map_err(|e| e.to_string())
    );

    say!("\n== OAuth loopback (PKCE) ==");
    for (label, tamper) in [("valid state", false), ("tampered state", true)] {
        let on_url = |auth_url: &str| {
            let auth_url = auth_url.to_string();
            thread::spawn(move || {
                let u = url::Url::parse(&auth_url).unwrap();
                let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
                println!(
                    "  [oauth] {}://{}{} challenge_method={} scope={} redirect={}",
                    u.scheme(),
                    u.host_str().unwrap(),
                    u.path(),
                    q["code_challenge_method"],
                    q["scope"],
                    q["redirect_uri"]
                );
                let redirect = &q["redirect_uri"];
                let _ = ureq::get(format!("{redirect}favicon.ico")).call();
                let state = if tamper { "forged".to_string() } else { q["state"].clone() };
                let page =
                    ureq::get(format!("{redirect}?code=fakecode&state={state}")).call().unwrap().body_mut().read_to_string().unwrap();
                let title = page.split("<title>").nth(1).and_then(|s| s.split("</title>").next()).unwrap_or_default();
                println!("  [oauth] browser sees page titled {title:?}");
            });
        };
        let opts = oauth::AuthorizeOptions { timeout: std::time::Duration::from_secs(20), on_url: Some(&on_url), port: 0 };
        let r = oauth::authorize_with(oauth::Provider::Dropbox, "snapoleon-smoke-client", None, &opts);
        say!("{label}: {:?}", r.map(|_| ()).map_err(|e| e.to_string()));
    }
    let timeout = oauth::authorize_with(
        oauth::Provider::Box,
        "x",
        None,
        &oauth::AuthorizeOptions { timeout: std::time::Duration::from_millis(300), on_url: Some(&|_| {}), port: 0 },
    );
    say!("no browser callback: {:?}", timeout.map(|_| ()).map_err(|e| e.to_string()));

    say!("\n== is.gd (live) ==");
    say!("{:?}", shorten("https://www.rust-lang.org/learn", &Shortener::IsGd, None, &secrets).map_err(|e| e.to_string()));

    say!("\n== Provider error paths (live, bogus credentials) ==");
    secrets.set("tg", "123456:not-a-real-token").unwrap();
    for (label, d) in [
        ("telegram", dest("tg", DestinationKind::Telegram { chat_id: "@snapoleon_test".into() }, Some("tg"))),
        ("imgur", dest("im", DestinationKind::Imgur { client_id: "0000000000000".into() }, None)),
    ] {
        say!("{label}: {:?}", uploader_for(&d, &secrets).and_then(|u| u.test()).map_err(|e| e.to_string()));
    }
    let missing = dest("dbx", DestinationKind::Dropbox { folder: "/Snapoleon".into() }, Some("nothing-stored"));
    say!("dropbox without credential: {:?}", uploader_for(&missing, &secrets).map(|_| ()).map_err(|e| e.to_string()));
    say!("\n== format_link ==");
    let url = "https://x.test/a b/(1).png";
    for (f, label) in
        [(LinkFormat::Url, "url"), (LinkFormat::Markdown, "markdown"), (LinkFormat::Html, "html"), (LinkFormat::BbCode, "bbcode")]
    {
        say!("{label:<9} link : {}", format_link(url, f, Some("Tom & <Jerry> [x]"), false));
        say!("{label:<9} image: {}", format_link(url, f, Some("Tom & <Jerry> [x]"), true));
    }

    say!("\n== i18n ==");
    for lang in ["en", "ru"] {
        snap_core::i18n::set_language(lang);
        say!("{lang}: {}", snap_core::tf("upload.error.http", &[("provider", "Telegram"), ("status", "401"), ("detail", "Unauthorized")]));
        say!("{lang}: {}", snap_core::t("upload.oauth.done_title"));
    }

    say!("\n== sxcu round trip ==");
    let sxcu_src = r#"{
  "Version": "13.2.1", "Name": "Legacy host", "DestinationType": "ImageUploader",
  "RequestMethod": "POST", "RequestURL": "https://up.example.com/api/upload",
  "Headers": { "Authorization": "Bearer $input$" }, "Body": "MultipartFormData",
  "Arguments": { "title": "$filename$" }, "FileFormName": "image",
  "URL": "$json:data.link$", "ThumbnailURL": "$json:data.thumb$", "DeletionURL": "$regex:1$",
  "RegexList": ["del=(\\w+)"], "ErrorMessage": "$json:error.message$"
}"#;
    let (name, imported) = sxcu::import(sxcu_src).unwrap();
    say!("imported {name:?}: {imported:#?}");
    let exported = sxcu::export(&name, &imported);
    say!("exported:\n{exported}");
    let (_, again) = sxcu::import(&exported).unwrap();
    say!("round trip identical: {}", again == imported);

    std::fs::write(out.join("report.txt"), report.into_inner()).unwrap();
}
