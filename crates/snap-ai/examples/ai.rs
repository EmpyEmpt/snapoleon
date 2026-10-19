//! Smoke test: request building and parsing against a local mock server, plus a
//! live title() call when an Ollama instance answers on 127.0.0.1:11434.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use snap_ai::{Ai, HowToStep};
use snap_core::{
    CaptureContext, Error, RgbaImage,
    secrets::{MemoryStore, SecretStore},
    settings::{AiSettings, TranslateProvider, TranslateSettings},
};

type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Value)>>>;

fn main() -> Result<(), Error> {
    let out = PathBuf::from("C:/Users/3mpti/snapoleon/target/smoke/snap-ai");
    std::fs::create_dir_all(&out)?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen: Seen = Arc::default();
    let server_seen = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = serve(stream, &server_seen);
        }
    });

    let secrets = MemoryStore::default();
    secrets.set("ai-key", "sk-test")?;
    let settings = AiSettings {
        enabled: true,
        endpoint: format!("http://127.0.0.1:{port}/v1"),
        model: "qwen3:8b".into(),
        vision_model: "qwen2.5vl:7b".into(),
        secret_ref: Some("ai-key".into()),
    };
    let ai = Ai::new(&settings, &secrets);
    println!("available: {}", ai.available());
    println!("test:\n{}\n", ai.test()?);

    let img = test_image(3000, 1800);
    let cx = CaptureContext { app_name: Some("Code".into()), window_title: Some("main.rs".into()), ..Default::default() };
    println!("alt_text: {:?}", ai.alt_text(&img, &cx)?);
    println!("title(img): {:?}", ai.title(Some(&img), None, &cx)?);
    println!("title(text): {:?}", ai.title(None, Some("Quarterly report draft"), &CaptureContext::default())?);
    let steps = vec![
        HowToStep { caption: "Open settings".into(), text: None, image: Some(test_image(800, 500)) },
        HowToStep { caption: "Enable AI".into(), text: Some("toggle at the top".into()), image: None },
        HowToStep { caption: "Press test".into(), text: None, image: None },
    ];
    println!("howto: {:#?}", ai.howto(&steps)?);
    let text = "Invoice for John Smith, contact john@example.com, total 120 USD";
    println!("redactions: {:?}", ai.suggest_redactions(text)?);
    println!("translate(ai): {:?}", ai.translate("Hello, world", "ru")?);

    let libre = TranslateSettings {
        provider: TranslateProvider::LibreTranslate { url: format!("http://127.0.0.1:{port}") },
        target_lang: "ru".into(),
        secret_ref: Some("ai-key".into()),
    };
    println!("translate(libre): {:?}", snap_ai::translate("Hello", "", &libre, &settings, &secrets)?);
    let deepl = TranslateSettings { provider: TranslateProvider::DeepL { free_api: true }, ..libre.clone() };
    let no_keys = MemoryStore::default();
    println!("translate(deepl, no key): {}", snap_ai::translate("Hello", "de", &deepl, &settings, &no_keys).unwrap_err());

    let denied = AiSettings { endpoint: format!("http://127.0.0.1:{port}/denied/v1"), secret_ref: None, ..settings.clone() };
    println!("denied: {}", Ai::new(&denied, &secrets).title(None, Some("x"), &cx).unwrap_err());
    let dead = AiSettings { endpoint: "http://127.0.0.1:1/v1".into(), ..settings.clone() };
    println!("unreachable: {}", Ai::new(&dead, &secrets).test().unwrap_err());
    let off = AiSettings { enabled: false, ..settings.clone() };
    println!("disabled: {}", Ai::new(&off, &secrets).alt_text(&img, &cx).unwrap_err());

    println!("\nrequests seen by the mock:");
    for (method, path, auth, body) in seen.lock().unwrap().iter() {
        println!("  {method} {path} auth={auth:?} {}", describe(body));
    }
    if let Some(jpeg) = seen.lock().unwrap().iter().find_map(|(_, _, _, b)| first_image(b)) {
        let decoded = image::load_from_memory(&jpeg)?;
        println!("first image sent: {}x{} JPEG, {} bytes", decoded.width(), decoded.height(), jpeg.len());
        std::fs::write(out.join("sent.jpg"), &jpeg)?;
    }

    if TcpStream::connect_timeout(&"127.0.0.1:11434".parse().unwrap(), Duration::from_millis(300)).is_ok() {
        let live = AiSettings { enabled: true, ..AiSettings::default() };
        let ai = Ai::new(&live, &secrets);
        println!("\nlive Ollama test: {:?}", ai.test());
        println!("live Ollama title: {:?}", ai.title(Some(&test_image(640, 400)), None, &cx));
    } else {
        println!("\nskip: no Ollama on 127.0.0.1:11434");
    }
    Ok(())
}

fn test_image(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let bar = y < h / 12;
        let stripe = (x / 40 + y / 40) % 2 == 0;
        let (r, g) = (x * 255 / w, y * 255 / h);
        if bar { image::Rgba([40, 40, 48, 255]) } else if stripe { image::Rgba([r as u8, g as u8, 200, 255]) } else { image::Rgba([r as u8, g as u8, 120, 128]) }
    })
}

fn describe(body: &Value) -> String {
    let model = body.get("model").and_then(Value::as_str).unwrap_or("-");
    let temp = body.get("temperature").map(Value::to_string).unwrap_or_else(|| "-".into());
    let user = body.pointer("/messages/1/content");
    let shape = match user {
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|p| p.get("type").and_then(Value::as_str).unwrap_or("?"))
            .collect::<Vec<_>>()
            .join("+"),
        Some(Value::String(_)) => "text".into(),
        _ => "-".into(),
    };
    format!("model={model} temperature={temp} user=[{shape}]")
}

fn first_image(body: &Value) -> Option<Vec<u8>> {
    let parts = body.pointer("/messages/1/content")?.as_array()?;
    let url = parts.iter().find_map(|p| p.pointer("/image_url/url")?.as_str())?;
    STANDARD.decode(url.strip_prefix("data:image/jpeg;base64,")?).ok()
}

fn serve(mut stream: TcpStream, seen: &Seen) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    let header_end = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (request_line.next().unwrap_or("").to_owned(), request_line.next().unwrap_or("").to_owned());
    let header = |name: &str| {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    };
    let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while buf.len() < header_end + length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body: Value = serde_json::from_slice(&buf[header_end..]).unwrap_or(Value::Null);
    seen.lock().unwrap().push((method.clone(), path.clone(), header("authorization"), body.clone()));

    let (status, reply) = match (method.as_str(), path.as_str()) {
        ("GET", "/v1/models") => (200, json!({"data": [{"id": "qwen3:8b"}, {"id": "llama3.2:latest"}]})),
        ("POST", "/translate") => (200, json!({"translatedText": "Привет"})),
        ("POST", "/denied/v1/chat/completions") => (401, json!({"error": {"message": "Incorrect API key provided"}})),
        ("POST", "/v1/chat/completions") => (200, chat_reply(&body)),
        _ => (404, json!({"error": "not found"})),
    };
    let payload = reply.to_string();
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    )
}

fn chat_reply(body: &Value) -> Value {
    let system = body.pointer("/messages/0/content").and_then(Value::as_str).unwrap_or("");
    let content = if system.contains("alt text") {
        "```\n\"A code editor showing a Rust file with a build output panel.\"\n```"
    } else if system.contains("name screenshots") {
        "<think>short and descriptive</think>\"Rust: editor / build output.\""
    } else if system.contains("how-to") {
        "Sure! Here it is:\n```json\n{\"title\": \"Enable AI\", \"intro\": \"Turn on AI in settings.\", \"steps\": [\"Open the settings window.\", {\"text\": \"Switch on the AI toggle.\"}]}\n```"
    } else if system.contains("hidden before") {
        "[\"John Smith\", \"john@example.com\", \"not present in the text\"]"
    } else {
        "Привет, мир"
    };
    json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": content}}]})
}
