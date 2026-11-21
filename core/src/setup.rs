//! First-run setup: connect to (or install) Navidrome, sign in to Soulseek, and fetch the helper
//! programs crate runs. Nothing here needs root: programs crate downloads go in
//! ~/.local/share/crate/bin, and a Navidrome it sets up runs as a systemd *user* service.
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Port a Navidrome set up by crate listens on (only on this computer).
pub const NAVIDROME_PORT: u16 = 4533;
const SLSK_SERVER: &str = "server.slsknet.org:2242";

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("crate")
}

/// Where crate keeps programs it downloaded itself.
pub fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

fn config_path() -> Result<PathBuf> {
    Ok(dirs::config_dir().context("no config folder")?.join("crate/config.json"))
}

/// Full path of a helper program: on PATH, or one crate downloaded.
pub fn tool_path(name: &str) -> Option<PathBuf> {
    let own = bin_dir().join(name);
    let on_path = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| is_exe(f)));
    on_path.or_else(|| is_exe(&own).then_some(own))
}

fn is_exe(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// PATH with crate's own bin folder added, for running helpers that run other helpers
/// (sockseek starts yt-dlp).
pub fn child_path() -> std::ffi::OsString {
    let mut dirs = vec![bin_dir()];
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

// ------------------------------------------------------------------ helper programs

#[derive(Clone, Debug)]
pub struct Tool {
    pub name: &'static str,
    pub what: &'static str,
    pub found: bool,
    /// crate downloads it into its own folder (no password needed).
    pub auto: bool,
    /// Installed with the system's package manager, which asks for your password.
    pub system: bool,
    /// The command to install it by hand, when crate can't do it for you.
    pub install: Option<String>,
}

/// The programs crate uses and whether they're installed.
pub fn tools() -> Vec<Tool> {
    let sys = |name: &'static str, what: &'static str| {
        let cmd = system_install(name);
        Tool {
            name,
            what,
            found: tool_path(name).is_some(),
            auto: false,
            system: cmd.is_some() && tool_path("pkexec").is_some(),
            install: Some(match &cmd {
                Some(args) => format!("sudo {}", args.join(" ")),
                None => format!("install \u{201c}{name}\u{201d} with your package manager"),
            }),
        }
    };
    let own = |name: &'static str, what: &'static str| Tool { name, what, found: tool_path(name).is_some(), auto: true, system: false, install: None };
    vec![
        sys("ffmpeg", "Converts downloads and measures loudness"),
        sys("fpcalc", "Recognises duplicate songs by how they sound"),
        own("sockseek", "Finds and downloads songs from Soulseek"),
        own("yt-dlp", "Downloads from YouTube when Soulseek doesn't have a song"),
    ]
}

/// The package-manager command that installs `tool` on this system (run as root).
fn system_install(tool: &str) -> Option<Vec<String>> {
    let pm = ["pacman", "apt-get", "dnf", "zypper"].into_iter().find(|p| tool_path(p).is_some())?;
    install_args(pm, tool)
}

fn install_args(pm: &str, tool: &str) -> Option<Vec<String>> {
    // package names differ between distros
    let pkg = match (tool, pm) {
        ("ffmpeg", "dnf") => "ffmpeg-free",
        ("ffmpeg", _) => "ffmpeg",
        ("fpcalc", "pacman") => "chromaprint",
        ("fpcalc", "apt-get") => "libchromaprint-tools",
        ("fpcalc", "dnf") => "chromaprint-tools",
        ("fpcalc", "zypper") => "chromaprint-fpcalc",
        _ => return None,
    };
    let args: &[&str] = match pm {
        "pacman" => &["pacman", "-S", "--needed", "--noconfirm"],
        "apt-get" => &["apt-get", "install", "-y"],
        "dnf" => &["dnf", "install", "-y"],
        "zypper" => &["zypper", "--non-interactive", "install"],
        _ => return None,
    };
    Some(args.iter().map(|s| s.to_string()).chain([pkg.to_string()]).collect())
}

/// Install ffmpeg or fpcalc with the package manager. pkexec shows the desktop's own password
/// window (the same one software centres use); crate never sees the password.
fn install_system(tool: &str) -> Result<()> {
    let args = system_install(tool).ok_or_else(|| anyhow!("crate doesn't know how to install {tool} on this system"))?;
    let pkexec = tool_path("pkexec").ok_or_else(|| anyhow!("can't ask for your password here (no pkexec); run: sudo {}", args.join(" ")))?;
    let prog = tool_path(&args[0]).unwrap_or_else(|| PathBuf::from(&args[0]));
    let out = Command::new(pkexec).arg(prog).args(&args[1..]).output()?;
    match out.status.code() {
        Some(0) if tool_path(tool).is_some() => Ok(()),
        Some(0) => bail!("installed, but {tool} still isn't found"),
        Some(126) => bail!("Cancelled."),
        Some(127) => bail!("Not allowed: wrong password, or this account can't install programs."),
        _ => {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("it failed");
            bail!("Couldn't install {tool}: {last}")
        }
    }
}

fn arch() -> Result<&'static str> {
    Ok(match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => bail!("no download for this kind of processor ({a})"),
    })
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .user_agent("crate (desktop music player)")
        .build()
        .expect("http client")
}

/// Latest release asset URL on GitHub whose name contains all of `parts`.
fn github_asset(repo: &str, parts: &[&str]) -> Result<String> {
    let v: Value = http().get(format!("https://api.github.com/repos/{repo}/releases/latest")).send()?.error_for_status()?.json()?;
    v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a["name"].as_str()?, a["browser_download_url"].as_str()?)))
        .find(|(n, _)| parts.iter().all(|p| n.contains(p)))
        .map(|(_, u)| u.to_string())
        .ok_or_else(|| anyhow!("no download for this computer in {repo}'s latest release"))
}

fn download(url: &str, to: &Path) -> Result<()> {
    let mut r = http().get(url).send()?.error_for_status()?;
    let tmp = to.with_extension("part");
    let mut f = std::fs::File::create(&tmp)?;
    std::io::copy(&mut r, &mut f)?;
    f.flush()?;
    std::fs::rename(&tmp, to)?;
    Ok(())
}

/// Download a release tarball and pull one program out of it into `dir`.
fn fetch_tar(url: &str, dir: &Path, program: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let tgz = dir.join(format!("{program}.tar.gz"));
    download(url, &tgz)?;
    let ok = Command::new("tar").arg("-xzf").arg(&tgz).arg("-C").arg(dir).arg(program).status()?.success();
    let _ = std::fs::remove_file(&tgz);
    if !ok {
        bail!("couldn't unpack {program}");
    }
    Ok(dir.join(program))
}

/// Install a helper: sockseek and yt-dlp are downloaded into crate's own folder; ffmpeg and
/// fpcalc come from the package manager (a password window pops up).
pub fn install_tool(name: &str) -> Result<()> {
    let dir = bin_dir();
    std::fs::create_dir_all(&dir)?;
    match name {
        "sockseek" => {
            let a = if arch()? == "amd64" { "linux-x64" } else { "linux-arm" };
            fetch_tar(&github_asset("fiso64/sockseek", &[a, ".tar.gz"])?, &dir, "sockseek")?;
        }
        "yt-dlp" => {
            let asset = if arch()? == "amd64" { "yt-dlp_linux" } else { "yt-dlp_linux_aarch64" };
            let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{asset}");
            download(&url, &dir.join("yt-dlp"))?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("yt-dlp"), std::fs::Permissions::from_mode(0o755))?;
        }
        "ffmpeg" | "fpcalc" => install_system(name)?,
        _ => bail!("crate can't install {name} by itself"),
    }
    Ok(())
}

// ------------------------------------------------------------------ Navidrome

/// The saved connection, if any: (server address, username, music folder).
pub fn current_login() -> Option<(String, String, String)> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(config_path().ok()?).ok()?).ok()?;
    Some((v["navidrome_url"].as_str()?.into(), v["user"].as_str()?.into(), v["library"].as_str().unwrap_or_default().into()))
}

fn token(password: &str) -> (String, String) {
    let salt = format!("{:016x}", rand::random::<u64>());
    (format!("{:x}", md5::compute(format!("{password}{salt}"))), salt)
}

/// Is there a Navidrome (or other Subsonic server) at `url`?
pub fn server_alive(url: &str) -> bool {
    let url = url.trim_end_matches('/');
    let r = http().get(format!("{url}/rest/ping")).query(&[("f", "json")]).timeout(Duration::from_secs(4)).send();
    r.ok().and_then(|r| r.json::<Value>().ok()).map(|v| v.get("subsonic-response").is_some()).unwrap_or(false)
}

/// Check a username and password against the server and save them (as a salted token, never the
/// password itself) along with the music folder.
pub fn login(url: &str, user: &str, password: &str, music: &Path) -> Result<()> {
    let mut url = url.trim().trim_end_matches('/').to_string();
    if !url.contains("://") {
        url = format!("http://{url}");
    }
    let (t, s) = token(password);
    let r: Value = http()
        .get(format!("{url}/rest/ping"))
        .query(&[("u", user), ("t", &t), ("s", &s), ("v", "1.16.1"), ("c", "crate-desktop"), ("f", "json")])
        .timeout(Duration::from_secs(10))
        .send()
        .with_context(|| format!("couldn't reach {url}"))?
        .json()
        .context("that address doesn't look like a Navidrome server")?;
    let body = &r["subsonic-response"];
    if body["status"] != "ok" {
        bail!("{}", body["error"]["message"].as_str().unwrap_or("wrong username or password"));
    }
    let path = config_path()?;
    let mut conf: Value = std::fs::read_to_string(&path).ok().and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(json!({}));
    conf["navidrome_url"] = json!(url);
    conf["user"] = json!(user);
    conf["token"] = json!(t);
    conf["salt"] = json!(s);
    conf["library"] = json!(music.to_string_lossy());
    write_private(&path, &serde_json::to_vec_pretty(&conf)?)
}

/// Write a file only this user can read (it holds a login).
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    f.flush()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Where a crate-installed Navidrome lives: program, settings and its database.
pub fn navidrome_dir() -> PathBuf {
    data_dir().join("navidrome")
}

const UNIT: &str = "crate-navidrome.service";

/// Download Navidrome, point it at `music`, and run it as a user service that starts when you
/// log in. Then create its first account. `step` reports what's happening.
pub fn install_navidrome(music: &Path, user: &str, password: &str, step: &dyn Fn(&str)) -> Result<String> {
    let url = format!("http://127.0.0.1:{NAVIDROME_PORT}");
    if server_alive(&url) {
        bail!("A Navidrome is already running on this computer ({url}). Choose \u{201c}I already have one\u{201d} and sign in to it instead.");
    }
    if tool_path("systemctl").is_none() {
        bail!("This system doesn't use systemd, so crate can't keep Navidrome running by itself. Install Navidrome yourself (navidrome.org) and choose \u{201c}I already have one\u{201d}.");
    }
    let dir = navidrome_dir();
    step("Downloading Navidrome…");
    let bin = prepare_navidrome(&dir, music, NAVIDROME_PORT)?;
    step("Setting it up…");
    write_unit(&bin, &dir)?;
    let sc = |args: &[&str]| Command::new("systemctl").arg("--user").args(args).status().map(|s| s.success()).unwrap_or(false);
    if !sc(&["daemon-reload"]) || !sc(&["enable", "--now", UNIT]) {
        bail!("couldn't start Navidrome (systemctl --user enable --now {UNIT} failed)");
    }
    step("Starting Navidrome…");
    for _ in 0..60 {
        if server_alive(&url) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !server_alive(&url) {
        bail!("Navidrome didn't start; see `journalctl --user -u {UNIT}`");
    }
    step("Creating your account…");
    create_admin(&url, user, password)?;
    login(&url, user, password, music)?;
    Ok(url)
}

/// Download Navidrome into `dir` and write its settings (music folder, database next to it,
/// reachable only from this computer). Returns the program's path.
pub fn prepare_navidrome(dir: &Path, music: &Path, port: u16) -> Result<PathBuf> {
    let asset = github_asset("navidrome/navidrome", &[&format!("_linux_{}.tar.gz", arch()?)])?;
    let bin = fetch_tar(&asset, dir, "navidrome")?;
    std::fs::create_dir_all(dir.join("data"))?;
    let toml = format!(
        "# Written by crate. Navidrome's options: https://www.navidrome.org/docs/usage/configuration-options/\n\
         MusicFolder = {music:?}\nDataFolder = {data:?}\nAddress = \"127.0.0.1\"\nPort = {port}\n",
        music = music.to_string_lossy(),
        data = dir.join("data").to_string_lossy(),
    );
    std::fs::write(dir.join("navidrome.toml"), toml)?;
    Ok(bin)
}

fn write_unit(bin: &Path, dir: &Path) -> Result<()> {
    let units = dirs::config_dir().context("no config folder")?.join("systemd/user");
    std::fs::create_dir_all(&units)?;
    let unit = format!(
        "[Unit]\nDescription=Navidrome music server (set up by crate)\nAfter=network.target\n\n\
         [Service]\nExecStart={bin:?} --configfile {conf:?}\nWorkingDirectory={dir:?}\nRestart=on-failure\n\n\
         [Install]\nWantedBy=default.target\n",
        bin = bin.to_string_lossy(),
        conf = dir.join("navidrome.toml").to_string_lossy(),
        dir = dir.to_string_lossy(),
    );
    std::fs::write(units.join(UNIT), unit)?;
    Ok(())
}

/// A brand-new Navidrome asks for its first (admin) account; this is the request its own web
/// page sends.
pub fn create_admin(url: &str, user: &str, password: &str) -> Result<()> {
    let r = http().post(format!("{url}/auth/createAdmin")).json(&json!({ "username": user, "password": password })).send()?;
    if !r.status().is_success() {
        let msg: Value = r.json().unwrap_or(Value::Null);
        bail!("Navidrome refused the new account: {}", msg["error"].as_str().unwrap_or("it may already have one"));
    }
    Ok(())
}

// ------------------------------------------------------------------ Soulseek

#[derive(Debug, PartialEq)]
pub enum SlskLogin {
    /// signed in (a new name is registered on first sign-in)
    Ok,
    /// the name exists and the password is different
    Taken,
    Refused(String),
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    b.extend((s.len() as u32).to_le_bytes());
    b.extend(s.as_bytes());
}

fn read_u32(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

/// Sign in to the Soulseek server once and log straight out. Soulseek has no sign-up page: the
/// first sign-in with a free name creates the account, with that password.
pub fn soulseek_login(user: &str, password: &str) -> Result<SlskLogin> {
    soulseek_login_at(SLSK_SERVER, user, password)
}

pub fn soulseek_login_at(server: &str, user: &str, password: &str) -> Result<SlskLogin> {
    use std::net::ToSocketAddrs;
    let addr = server.to_socket_addrs()?.next().context("can't find the Soulseek server")?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).context("can't reach the Soulseek server")?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut body = vec![];
    body.extend(1u32.to_le_bytes()); // Login
    put_str(&mut body, user);
    put_str(&mut body, password);
    body.extend(160u32.to_le_bytes()); // client version
    put_str(&mut body, &format!("{:x}", md5::compute(format!("{user}{password}"))));
    body.extend(1u32.to_le_bytes()); // minor version
    let mut msg = (body.len() as u32).to_le_bytes().to_vec();
    msg.extend(body);
    s.write_all(&msg)?;
    loop {
        let len = read_u32(&mut s)? as usize;
        if len < 4 || len > 1 << 20 {
            bail!("unexpected reply from the Soulseek server");
        }
        let mut b = vec![0u8; len];
        s.read_exact(&mut b)?;
        if u32::from_le_bytes(b[..4].try_into().unwrap()) != 1 {
            continue; // something else; the login reply comes first in practice
        }
        let ok = b.get(4).copied().unwrap_or(0) == 1;
        if ok {
            return Ok(SlskLogin::Ok);
        }
        let reason = b.get(9..).map(|r| String::from_utf8_lossy(r).into_owned()).unwrap_or_default();
        return Ok(match reason.as_str() {
            "INVALIDPASS" => SlskLogin::Taken,
            "INVALIDUSERNAME" => SlskLogin::Refused("That name can't be used. Try letters and numbers only.".into()),
            "SVRFULL" | "SVRPRIVATE" => SlskLogin::Refused("The Soulseek server isn't taking sign-ins right now. Try again later.".into()),
            r => SlskLogin::Refused(format!("Soulseek said no ({r})")),
        });
    }
}

fn sockseek_conf() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("sockseek/sockseek.conf"))
}

/// The Soulseek name sockseek is set up with, if any.
pub fn soulseek_account() -> Option<String> {
    let raw = std::fs::read_to_string(sockseek_conf()?).ok()?;
    conf_value(&raw, "username")
}

fn conf_value(raw: &str, key: &str) -> Option<String> {
    raw.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string()).filter(|v| !v.is_empty())
    })
}

/// Replace (or add) `key = value` lines, keeping the rest of the file as it is.
fn conf_set(raw: &str, pairs: &[(&str, &str)]) -> String {
    let mut out: Vec<String> = vec![];
    let mut done = vec![false; pairs.len()];
    for l in raw.lines() {
        let key = l.split_once('=').map(|(k, _)| k.trim());
        match pairs.iter().position(|(k, _)| Some(*k) == key) {
            Some(i) if !done[i] => {
                out.push(format!("{} = {}", pairs[i].0, pairs[i].1));
                done[i] = true;
            }
            Some(_) => {}
            None => out.push(l.to_string()),
        }
    }
    for (i, (k, v)) in pairs.iter().enumerate() {
        if !done[i] {
            out.push(format!("{k} = {v}"));
        }
    }
    out.join("\n") + "\n"
}

/// Save the Soulseek sign-in where sockseek reads it (other settings in that file are kept).
pub fn save_soulseek(user: &str, password: &str) -> Result<()> {
    let path = sockseek_conf().context("no config folder")?;
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    write_private(&path, conf_set(&raw, &[("username", user), ("password", password)]).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_commands() {
        assert_eq!(install_args("pacman", "fpcalc").unwrap().join(" "), "pacman -S --needed --noconfirm chromaprint");
        assert_eq!(install_args("apt-get", "fpcalc").unwrap().join(" "), "apt-get install -y libchromaprint-tools");
        assert_eq!(install_args("dnf", "ffmpeg").unwrap().join(" "), "dnf install -y ffmpeg-free");
        assert!(install_args("pacman", "yt-dlp").is_none());
    }

    #[test]
    fn conf_edit_keeps_other_lines() {
        let raw = "username = old\n# note\npref-format = mp3\n";
        let out = conf_set(raw, &[("username", "new"), ("password", "pw")]);
        assert_eq!(out, "username = new\n# note\npref-format = mp3\npassword = pw\n");
        assert_eq!(conf_value(&out, "username").as_deref(), Some("new"));
    }

    /// A stand-in Soulseek server that answers like the real one.
    fn fake_server(reply_ok: bool, reason: &str) -> (String, std::thread::JoinHandle<(String, String)>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let reason = reason.to_string();
        let h = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let len = read_u32(&mut c).unwrap() as usize;
            let mut b = vec![0u8; len];
            c.read_exact(&mut b).unwrap();
            let s = |at: usize| {
                let n = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
                (String::from_utf8(b[at + 4..at + 4 + n].to_vec()).unwrap(), at + 4 + n)
            };
            let (user, next) = s(4);
            let (pass, _) = s(next);
            let mut body = 1u32.to_le_bytes().to_vec();
            body.push(reply_ok as u8);
            put_str(&mut body, if reply_ok { "welcome" } else { &reason });
            let mut msg = (body.len() as u32).to_le_bytes().to_vec();
            msg.extend(body);
            c.write_all(&msg).unwrap();
            (user, pass)
        });
        (addr, h)
    }

    #[test]
    fn soulseek_replies() {
        let (a, h) = fake_server(true, "");
        assert_eq!(soulseek_login_at(&a, "me", "pw").unwrap(), SlskLogin::Ok);
        assert_eq!(h.join().unwrap(), ("me".into(), "pw".into()));
        let (a, _) = fake_server(false, "INVALIDPASS");
        assert_eq!(soulseek_login_at(&a, "me", "x").unwrap(), SlskLogin::Taken);
    }
}
