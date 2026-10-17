//! gray-voicepro — cloud speech tools: vtts_say (Deepgram Aura or Fish Audio
//! TTS → wav|mp3 under ~/.gray/voicepro/out, played via aplay/paplay/ffplay/
//! mpg123 when present) and vstt_listen (mic capture via arecord/rec/sox →
//! Deepgram nova-3). Keys: DEEPGRAM_API_KEY / FISH_API_KEY env or
//! ~/.gray/voicepro/config.json (`voicepro setup`). HTTP is shelled to curl.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const NAME: &str = "voicepro";
const DG_SPEAK: &str = "https://api.deepgram.com/v1/speak";
const DG_LISTEN: &str = "https://api.deepgram.com/v1/listen";
const FISH_TTS: &str = "https://api.fish.audio/v1/tts";

fn manifest() -> Value {
    json!({
        "name": NAME, "display_name": "Voice Pro",
        "version": env!("CARGO_PKG_VERSION"), "protocol": "1.1",
        "tools": [
            {"name": "vtts_say",
             "description": "Speak text via cloud TTS — Deepgram Aura (DEEPGRAM_API_KEY) or Fish Audio (FISH_API_KEY). Saves wav/mp3 to ~/.gray/voicepro/out and plays it via aplay/paplay/ffplay/mpg123 when present.",
             "parameters": {"type": "object", "properties": {
                "text": {"type": "string"},
                "provider": {"type": "string", "description": "deepgram|fish; default: whichever key is set (deepgram first)"},
                "model": {"type": "string", "description": "deepgram aura voice (default aura-2-thalia-en) or fish reference_id"},
                "format": {"type": "string", "description": "wav (default) | mp3"},
                "play": {"type": "boolean", "description": "play the file after writing (default true)"}},
             "required": ["text"]}},
            {"name": "vstt_listen",
             "description": "Record N seconds from the mic (arecord/rec/sox) and transcribe with Deepgram nova-3 (DEEPGRAM_API_KEY).",
             "parameters": {"type": "object", "properties": {
                "seconds": {"type": "integer", "description": "1-60, default 5"},
                "model": {"type": "string", "description": "deepgram listen model, default nova-3"},
                "language": {"type": "string", "description": "BCP-47, e.g. en or multi"}},
             }},
        ],
        "commands": ["/voicepro"], "subcommands": ["voicepro"],
    })
}

// ---------- state + config ---------------------------------------------------

fn state_dir() -> PathBuf {
    let home = std::env::var_os("GRAY_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".gray")))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(NAME)
}
fn config_path() -> PathBuf {
    state_dir().join("config.json")
}
fn out_dir() -> PathBuf {
    state_dir().join("out")
}
fn read_config() -> Value {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}
fn env_var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.trim().is_empty())
}
fn cfg(key: &str, env: &str) -> Option<String> {
    env_var(env).or_else(|| read_config()[key].as_str().map(str::to_string))
}
fn dg_key() -> Option<String> {
    cfg("deepgram_api_key", "DEEPGRAM_API_KEY")
}
fn fish_key() -> Option<String> {
    cfg("fish_api_key", "FISH_API_KEY")
}

// ---------- binaries ---------------------------------------------------------

fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || Command::new("which")
            .arg(bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}
fn pick(cands: &[&str]) -> Option<String> {
    cands.iter().find(|b| have(b)).map(|s| s.to_string())
}

// ---------- http (via curl) ---------------------------------------------------

fn curl(args: &[String]) -> Result<std::process::Output, String> {
    if !have("curl") {
        return Err("curl not found — install curl".into());
    }
    Command::new("curl")
        .args(args)
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))
}

fn api_err(code: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/err_msg")
                .or_else(|| v.pointer("/error/message"))
                .or_else(|| v.pointer("/message"))
                .or_else(|| v.pointer("/detail"))
                .map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string()))
        })
        .unwrap_or_else(|| body.chars().take(250).collect());
    format!("HTTP {code}: {detail}")
}

// POST json, save response bytes to `path`. Returns bytes written.
fn fetch_audio(url: &str, headers: &[String], body: &Value, path: &Path) -> Result<u64, String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let mut a: Vec<String> = ["-sS", "--max-time", "90", "-X", "POST", "-o"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.push(path.display().to_string());
    a.extend(["-w".into(), "%{http_code}".into()]);
    for h in headers {
        a.extend(["-H".into(), h.clone()]);
    }
    a.extend([
        "-H".into(),
        "Content-Type: application/json".into(),
        "-d".into(),
        body.to_string(),
        url.into(),
    ]);
    let out = curl(&a)?;
    if !out.status.success() {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "curl: {}",
            String::from_utf8_lossy(&out.stderr).trim().chars().take(200).collect::<String>()
        ));
    }
    let code: u16 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    if !(200..300).contains(&code) {
        let detail = std::fs::read_to_string(path).unwrap_or_default();
        let _ = std::fs::remove_file(path);
        return Err(api_err(code, &detail));
    }
    let n = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if n == 0 {
        let _ = std::fs::remove_file(path);
        return Err("provider returned empty audio".into());
    }
    Ok(n)
}

// ---------- tools ------------------------------------------------------------

fn play(path: &Path, format: &str) -> Result<String, String> {
    let cands: &[(&str, &[&str])] = if format == "wav" {
        &[
            ("aplay", &["-q"]),
            ("paplay", &[]),
            ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "error"]),
        ]
    } else {
        &[
            ("mpg123", &["-q"]),
            ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "error"]),
            ("paplay", &[]),
        ]
    };
    for (bin, args) in cands {
        if !have(bin) {
            continue;
        }
        let ok = Command::new(bin)
            .args(*args)
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok((*bin).into());
        }
    }
    Err("no player — install alsa-utils (aplay), pulseaudio (paplay), ffmpeg (ffplay) or mpg123".into())
}

fn vtts_say(a: &Value) -> Result<String, String> {
    let text = a["text"].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("text required")?;
    let format = a["format"].as_str().unwrap_or("wav");
    if !matches!(format, "wav" | "mp3") {
        return Err(format!("format must be wav|mp3 (got {format})"));
    }
    let provider = match a["provider"].as_str() {
        Some(p) => p.to_string(),
        None => match (dg_key().is_some(), fish_key().is_some()) {
            (true, _) => "deepgram".into(),
            (false, true) => "fish".into(),
            (false, false) => {
                return Err("no TTS provider key — export DEEPGRAM_API_KEY or FISH_API_KEY (or `voicepro setup`)".into())
            }
        },
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let path = out_dir().join(format!("tts-{ts}.{format}"));
    let bytes = match provider.as_str() {
        "deepgram" => {
            let key = dg_key()
                .ok_or("no Deepgram key — export DEEPGRAM_API_KEY or `voicepro setup --deepgram-key …`")?;
            let model = a["model"]
                .as_str()
                .map(str::to_string)
                .or_else(|| env_var("DEEPGRAM_TTS_MODEL"))
                .unwrap_or_else(|| "aura-2-thalia-en".into());
            let mut url = format!("{DG_SPEAK}?model={model}&encoding={format}");
            if format == "wav" {
                url.push_str("&container=wav");
            }
            fetch_audio(&url, &[format!("Authorization: Token {key}")], &json!({"text": text}), &path)?
        }
        "fish" => {
            let key = fish_key()
                .ok_or("no Fish Audio key — export FISH_API_KEY or `voicepro setup --fish-key …`")?;
            let mut b = json!({"text": text, "format": format});
            if let Some(v) = a["model"].as_str() {
                b["reference_id"] = json!(v);
            }
            let model = env_var("FISH_MODEL").unwrap_or_else(|| "s2.1-pro".into());
            fetch_audio(
                FISH_TTS,
                &[format!("Authorization: Bearer {key}"), format!("model: {model}")],
                &b,
                &path,
            )?
        }
        other => return Err(format!("unknown provider {other} — use deepgram|fish")),
    };
    let msg = format!("{provider} → {} ({bytes} bytes)", path.display());
    if !a["play"].as_bool().unwrap_or(true) {
        return Ok(format!("{msg} (playback off)"));
    }
    match play(&path, format) {
        Ok(p) => Ok(format!("{msg}, played via {p}")),
        Err(e) => Ok(format!("{msg}; not played: {e}")),
    }
}

fn vstt_listen(a: &Value) -> Result<String, String> {
    let key = dg_key()
        .ok_or("no Deepgram key — export DEEPGRAM_API_KEY or `voicepro setup --deepgram-key …`")?;
    let secs = a["seconds"].as_u64().unwrap_or(5).clamp(1, 60).to_string();
    let wav = std::env::temp_dir().join(format!("voicepro-rec-{}.wav", std::process::id()));
    let rec = if have("arecord") {
        Command::new("arecord")
            .args(["-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-d", &secs])
            .arg(&wav)
            .status()
    } else if have("rec") {
        Command::new("rec")
            .args(["-q", "-r", "16000", "-c", "1"])
            .arg(&wav)
            .args(["trim", "0", &secs])
            .status()
    } else if have("sox") {
        Command::new("sox")
            .args(["-q", "-d", "-r", "16000", "-c", "1"])
            .arg(&wav)
            .args(["trim", "0", &secs])
            .status()
    } else {
        return Err("no recorder — install alsa-utils (arecord) or sox".into());
    };
    match rec {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(format!("recorder exited {s}")),
        Err(e) => return Err(format!("recorder spawn: {e}")),
    }
    let model = a["model"]
        .as_str()
        .map(str::to_string)
        .or_else(|| env_var("DEEPGRAM_STT_MODEL"))
        .unwrap_or_else(|| "nova-3".into());
    let mut url = format!("{DG_LISTEN}?model={model}&smart_format=true");
    if let Some(l) = a["language"].as_str() {
        url.push_str(&format!("&language={l}"));
    }
    let args: Vec<String> = [
        "-sS", "--max-time", "90", "-X", "POST", &url,
        "-H", &format!("Authorization: Token {key}"),
        "-H", "Content-Type: audio/wav",
        "--data-binary", &format!("@{}", wav.display()),
        "-w", "\n%{http_code}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let out = curl(&args);
    let _ = std::fs::remove_file(&wav);
    let out = out?;
    if !out.status.success() {
        return Err(format!(
            "curl: {}",
            String::from_utf8_lossy(&out.stderr).trim().chars().take(200).collect::<String>()
        ));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let (body, code) = s.rsplit_once('\n').unwrap_or((s.as_ref(), "0"));
    let code: u16 = code.trim().parse().unwrap_or(0);
    if !(200..300).contains(&code) {
        return Err(api_err(code, body));
    }
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad deepgram reply: {e}"))?;
    let t = v["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if t.is_empty() { Ok("(no speech detected)".into()) } else { Ok(t) }
}

fn tool(name: &str, args: &Value) -> Value {
    let r = match name {
        "vtts_say" => vtts_say(args),
        "vstt_listen" => vstt_listen(args),
        t => Err(format!("unknown tool {t}")),
    };
    match r {
        Ok(c) => json!({"result": {"content": c}}),
        Err(e) => json!({"result": {"content": e, "is_error": true}}),
    }
}

// ---------- cli / command -----------------------------------------------------

fn status() -> String {
    format!(
        "voicepro\n  deepgram key: {}\n  fish key: {}\n  recorder: {}\n  player: {}\n  curl: {}\n  out: {}",
        if dg_key().is_some() { "set" } else { "MISSING (DEEPGRAM_API_KEY)" },
        if fish_key().is_some() { "set" } else { "MISSING (FISH_API_KEY)" },
        pick(&["arecord", "rec", "sox"]).unwrap_or("none — install alsa-utils or sox".into()),
        pick(&["aplay", "paplay", "ffplay", "mpg123"]).unwrap_or("none".into()),
        if have("curl") { "ok" } else { "MISSING — install curl" },
        out_dir().display(),
    )
}

fn cli(argv: &[String]) -> Result<String, String> {
    match argv.first().map(String::as_str) {
        Some("setup") => {
            let mut c = read_config();
            let mut it = argv[1..].iter();
            while let Some(f) = it.next() {
                match f.as_str() {
                    "--deepgram-key" => {
                        c["deepgram_api_key"] = json!(it.next().ok_or("--deepgram-key needs a value")?)
                    }
                    "--fish-key" => {
                        c["fish_api_key"] = json!(it.next().ok_or("--fish-key needs a value")?)
                    }
                    o => return Err(format!("unknown flag {o}")),
                }
            }
            std::fs::create_dir_all(config_path().parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(config_path(), serde_json::to_string_pretty(&c).unwrap())
                .map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(config_path(), std::fs::Permissions::from_mode(0o600));
            }
            Ok(format!("saved {}", config_path().display()))
        }
        Some("status") | None => Ok(status()),
        Some("-h" | "--help" | "help") => Ok("usage: voicepro setup | status".into()),
        Some(o) => Err(format!("unknown verb {o}")),
    }
}

// ---------- wire ---------------------------------------------------------------

fn sidecar() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let method = v["method"].as_str().unwrap_or("");
        match v.get("id").cloned() {
            None if method == "plugin/shutdown" => return Ok(()),
            None => {}
            Some(id) => {
                let reply = if method == "plugin/manifest" {
                    json!({"id": id, "result": manifest()})
                } else if method == "tool/call" {
                    let mut r = tool(
                        v["params"]["name"].as_str().unwrap_or(""),
                        &v["params"]["args"],
                    );
                    r["id"] = id;
                    r
                } else if method == "command/run" {
                    let argv: Vec<String> = v["params"]["argv"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    let text = cli(&argv).unwrap_or_else(|e| e);
                    json!({"id": id, "result": {"text": text}})
                } else {
                    json!({"id": id, "error": {"code": -32601,
                        "message": format!("method not found: {method}")}})
                };
                writeln!(out, "{reply}")?;
                out.flush()?;
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<String> = match args.first().map(String::as_str) {
        Some("voicepro") => args[1..].to_vec(),
        _ => args,
    };
    match args.first().map(String::as_str) {
        Some("manifest") => println!("{}", manifest()),
        Some("sidecar") => {
            let _ = sidecar();
        }
        Some("status") => println!("{}", status()),
        _ => {
            if args.is_empty() {
                if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    println!("{}", status());
                    return;
                }
                let _ = sidecar();
                return;
            }
            match cli(&args) {
                Ok(t) => println!("{t}"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    struct TempHome;
    impl TempHome {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("gray-voicepro-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            unsafe { std::env::set_var("GRAY_HOME", &dir) };
            for k in ["DEEPGRAM_API_KEY", "FISH_API_KEY"] {
                unsafe { std::env::remove_var(k) };
            }
            Self
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            let dir = std::env::temp_dir().join(format!("gray-voicepro-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn manifest_shape() {
        let m = manifest();
        assert_eq!(m["name"], "voicepro");
        let names: Vec<&str> = m["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, ["vtts_say", "vstt_listen"]);
        assert_eq!(m["commands"], json!(["/voicepro"]));
    }

    #[test]
    fn say_requires_text() {
        let _g = LOCK.lock().unwrap();
        let _h = TempHome::new();
        let r = tool("vtts_say", &json!({}));
        assert_eq!(r["result"]["is_error"], true);
    }

    #[test]
    fn say_without_key_hints() {
        let _g = LOCK.lock().unwrap();
        let _h = TempHome::new();
        let r = tool("vtts_say", &json!({"text": "hi"}));
        assert_eq!(r["result"]["is_error"], true);
        assert!(r["result"]["content"].as_str().unwrap().contains("DEEPGRAM_API_KEY"));
    }

    #[test]
    fn listen_without_key_hints() {
        let _g = LOCK.lock().unwrap();
        let _h = TempHome::new();
        let r = tool("vstt_listen", &json!({"seconds": 1}));
        assert_eq!(r["result"]["is_error"], true);
        assert!(r["result"]["content"].as_str().unwrap().contains("DEEPGRAM_API_KEY"));
    }
}
