//! Bug and crash reports. The logger keeps the last lines of the log, a panic hook writes a
//! crash report into the data folder, and both kinds of report become prefilled GitHub
//! issues (`.github/ISSUE_TEMPLATE`). Nothing is sent by itself: the app shows the report
//! and opens the issue in the browser only on request.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Where reports go: the repository's "new issue" page.
const NEW_ISSUE: &str = "https://github.com/trueSilentisgold/inkpainter3d/issues/new";
/// Log lines a report carries.
pub const LOG_LINES: usize = 200;
/// GitHub refuses longer URLs; prefilled fields are cut to fit.
const MAX_URL: usize = 7500;

static RECENT_LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static GPU: OnceLock<String> = OnceLock::new();
static PROJECT: Mutex<String> = Mutex::new(String::new());
static START: OnceLock<Instant> = OnceLock::new();

/// env_logger's output, plus a copy of the last [`LOG_LINES`] lines for reports.
struct Logger {
    inner: env_logger::Logger,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if !self.inner.matches(record) {
            return;
        }
        self.inner.log(record);
        let elapsed = START.get().map_or(0.0, |s| s.elapsed().as_secs_f64());
        let line = format!(
            "[{elapsed:9.3}s {:5} {}] {}",
            record.level(),
            record.target(),
            record.args()
        );
        if let Ok(mut lines) = RECENT_LOG.lock() {
            if lines.len() == LOG_LINES {
                lines.pop_front();
            }
            lines.push_back(line);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Starts logging (`RUST_LOG` as usual) and the crash reports.
pub fn init() {
    START.get_or_init(Instant::now);
    let inner = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"),
    )
    .build();
    let level = inner.filter();
    if log::set_boxed_logger(Box::new(Logger { inner })).is_ok() {
        log::set_max_level(level);
    }
    install_panic_hook();
}

/// Writes a crash report before the default hook prints the panic.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_owned());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_owned());
        let thread = std::thread::current();
        let crash = Crash {
            message,
            location,
            thread: thread.name().unwrap_or("unnamed").to_owned(),
            backtrace: trim_backtrace(&std::backtrace::Backtrace::force_capture().to_string()),
        };
        match crash.write() {
            Some(path) => eprintln!("inkpainter3d: crash report written to {}", path.display()),
            None => eprintln!("inkpainter3d: could not write a crash report"),
        }
        default(info);
    }));
}

/// The GPU the app runs on, for reports.
pub fn set_gpu(info: &ink_render::wgpu::AdapterInfo) {
    let driver = [info.driver.as_str(), info.driver_info.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let _ = GPU.set(format!(
        "{} ({:?}, {:?}), driver {}",
        info.name,
        info.backend,
        info.device_type,
        if driver.is_empty() {
            "unknown"
        } else {
            &driver
        }
    ));
}

/// What the open project looks like, without names, paths or pixels.
pub fn set_project(model: &ink_core::Model) {
    let sets = model
        .texture_sets
        .iter()
        .map(|s| {
            format!(
                "{}² with {}",
                s.resolution,
                count(s.stack.iter().count(), "layer")
            )
        })
        .collect::<Vec<_>>();
    let summary = format!(
        "{} ({}), {}, {}",
        count(sets.len(), "texture set"),
        sets.join(", "),
        count(model.objects.len(), "mesh"),
        count(model.mesh.indices.len() / 3, "triangle")
    );
    if let Ok(mut project) = PROJECT.lock()
        && *project != summary
    {
        *project = summary;
    }
}

/// `1 layer`, `2 layers`, `3 meshes`.
fn count(n: usize, noun: &str) -> String {
    let plural = if n == 1 {
        ""
    } else if noun.ends_with("sh") {
        "es"
    } else {
        "s"
    };
    format!("{n} {noun}{plural}")
}

/// Version, operating system, GPU and project, one per line.
pub fn system_info() -> String {
    let project = PROJECT.lock().map(|p| p.clone()).unwrap_or_default();
    format!(
        "Version: {}\nOS: {} ({})\nGPU: {}\nProject: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        GPU.get().map_or("unknown", String::as_str),
        if project.is_empty() { "none" } else { &project }
    )
}

/// The last lines of the log, oldest first.
pub fn recent_log() -> String {
    RECENT_LOG
        .lock()
        .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// The backtrace from the code that panicked on: the frames of the backtrace capture and
/// the panic machinery above it go.
fn trim_backtrace(backtrace: &str) -> String {
    let lines: Vec<&str> = backtrace.lines().collect();
    let is_frame = |line: &str| {
        let line = line.trim_start();
        line.split_once(':')
            .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    // Where the panic entered the runtime; later frames such as `catch_unwind` in
    // `lang_start` belong to the caller.
    let entry = |line: &str| {
        is_frame(line)
            && [
                "core::panicking::",
                "rust_begin_unwind",
                "std::panicking::begin_panic",
            ]
            .iter()
            .any(|p| line.contains(p))
    };
    lines
        .iter()
        .position(|l| entry(l))
        .and_then(|first| {
            (first + 1..lines.len()).find(|&i| is_frame(lines[i]) && !entry(lines[i]))
        })
        .map_or_else(|| backtrace.to_owned(), |start| lines[start..].join("\n"))
}

struct Crash {
    message: String,
    location: String,
    thread: String,
    backtrace: String,
}

impl Crash {
    fn write(&self) -> Option<PathBuf> {
        let dir = crash_dir()?;
        std::fs::create_dir_all(&dir).ok()?;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut text = String::new();
        let _ = writeln!(text, "{CRASH_HEADER}");
        let _ = writeln!(text, "Time: {}\n", utc_timestamp(secs));
        let _ = writeln!(text, "{SECTION}Panic");
        let _ = writeln!(
            text,
            "{}\nat {} (thread '{}')\n",
            self.message, self.location, self.thread
        );
        let _ = writeln!(text, "{SECTION}System");
        let _ = writeln!(text, "{}\n", system_info());
        let _ = writeln!(text, "{SECTION}Backtrace");
        let _ = writeln!(text, "{}\n", self.backtrace.trim_end());
        let _ = writeln!(text, "{SECTION}Log");
        let _ = writeln!(text, "{}", recent_log());
        let path = dir.join(format!("crash-{secs}.txt"));
        std::fs::write(&path, text).ok()?;
        Some(path)
    }
}

const CRASH_HEADER: &str = "inkpainter3d crash report";
const SECTION: &str = "## ";

/// `<data folder>/crashes`; reports already answered move to `crashes/seen`.
pub fn crash_dir() -> Option<PathBuf> {
    crate::app::data_dir().map(|d| d.join("crashes"))
}

/// A crash report from an earlier run that the user hasn't answered yet.
pub struct CrashReport {
    pub path: PathBuf,
    pub text: String,
}

impl CrashReport {
    /// The newest unanswered report, if any.
    pub fn pending() -> Option<Self> {
        let dir = crash_dir()?;
        let path = std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|e| e == "txt")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("crash-"))
            })
            .max()?;
        let text = std::fs::read_to_string(&path).ok()?;
        Some(Self { path, text })
    }

    /// Moves this report and any older ones out of the way, so the question comes once.
    pub fn mark_seen(&self) {
        let Some(dir) = self.path.parent() else {
            return;
        };
        let seen = dir.join("seen");
        if std::fs::create_dir_all(&seen).is_err() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
                if path.is_file()
                    && let Some(name) = path.file_name()
                {
                    let _ = std::fs::rename(&path, seen.join(name));
                }
            }
        }
    }

    /// One section of the report, without its heading.
    fn section(&self, name: &str) -> &str {
        let heading = format!("{SECTION}{name}\n");
        let Some(start) = self.text.find(&heading) else {
            return "";
        };
        let rest = &self.text[start + heading.len()..];
        let end = rest.find(&format!("\n{SECTION}")).unwrap_or(rest.len());
        rest[..end].trim()
    }

    /// The panic message, for the issue title.
    pub fn message(&self) -> &str {
        self.section("Panic").lines().next().unwrap_or("")
    }

    /// The prefilled crash issue.
    pub fn issue_url(&self) -> String {
        let title = format!("Crash: {}", self.message());
        issue_url(
            &title,
            &[
                Part::text(
                    "What were you doing?",
                    "(What you did right before the crash, as far as you remember.)",
                ),
                Part::code("Version and system", self.section("System")),
                Part::code("Error", self.section("Panic")),
                Part::code("Backtrace", self.section("Backtrace")),
                Part::log(self.section("Log")),
            ],
        )
    }
}

/// The prefilled bug issue.
pub fn bug_issue_url(title: &str, description: &str, include_log: bool) -> String {
    let log = if include_log {
        recent_log()
    } else {
        String::new()
    };
    issue_url(
        title,
        &[
            Part::text("What happened?", description),
            Part::code("Version and system", &system_info()),
            Part::log(&log),
        ],
    )
}

/// The full text of a bug report, for the clipboard.
pub fn bug_report_text(title: &str, description: &str, include_log: bool) -> String {
    let mut text = format!(
        "{title}\n\n{description}\n\n{SECTION}System\n{}\n",
        system_info()
    );
    if include_log {
        let _ = write!(text, "\n{SECTION}Log\n{}\n", recent_log());
    }
    text
}

/// One section of an issue's text.
struct Part {
    heading: &'static str,
    text: String,
    /// Shown as a code block.
    code: bool,
    /// Gives up its oldest lines first when the issue gets too long, not its last.
    log: bool,
}

impl Part {
    fn text(heading: &'static str, text: &str) -> Self {
        Self {
            heading,
            text: text.to_owned(),
            code: false,
            log: false,
        }
    }

    fn code(heading: &'static str, text: &str) -> Self {
        Self {
            heading,
            text: text.to_owned(),
            code: true,
            log: false,
        }
    }

    fn log(text: &str) -> Self {
        Self {
            heading: "Log",
            text: text.to_owned(),
            code: true,
            log: true,
        }
    }
}

/// A "new issue" URL with the title and the text filled in, as Markdown in the body, so it
/// works without the issue forms. Parts that would make the URL too long lose lines first
/// (the log its oldest, the others their last); the full text is on the clipboard or in the
/// crash file.
fn issue_url(title: &str, parts: &[Part]) -> String {
    let mut parts: Vec<Part> = parts
        .iter()
        .filter(|p| !p.text.trim().is_empty())
        .map(|p| Part {
            text: p.text.trim().to_owned(),
            ..*p
        })
        .collect();
    let build = |parts: &[Part]| {
        let mut body = String::new();
        for part in parts {
            let _ = write!(body, "### {}\n\n", part.heading);
            if part.code {
                let _ = write!(body, "```text\n{}\n```\n\n", part.text);
            } else {
                let _ = write!(body, "{}\n\n", part.text);
            }
        }
        format!(
            "{NEW_ISSUE}?labels=bug&title={}&body={}",
            encode(title),
            encode(body.trim_end())
        )
    };
    let mut url = build(&parts);
    let marker = "[… cut, see the full report …]";
    while url.len() > MAX_URL {
        let Some(part) = parts.iter_mut().max_by_key(|p| encode(&p.text).len()) else {
            break;
        };
        let mut lines: Vec<&str> = part.text.lines().filter(|l| *l != marker).collect();
        if lines.len() <= 2 {
            let keep = part.text.chars().count() / 2;
            part.text = part.text.chars().take(keep).collect::<String>() + "…";
        } else if part.log {
            lines.drain(..lines.len().div_ceil(4));
            part.text = std::iter::once(marker)
                .chain(lines)
                .collect::<Vec<_>>()
                .join("\n");
        } else {
            lines.truncate(lines.len() - lines.len().div_ceil(4));
            part.text = lines
                .into_iter()
                .chain(std::iter::once(marker))
                .collect::<Vec<_>>()
                .join("\n");
        }
        url = build(&parts);
    }
    url
}

/// Percent-encoding for a query value.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// `2026-10-22 14:03:12 UTC` from seconds since 1970.
fn utc_timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Opens a folder in the system's file manager.
pub fn open_folder(dir: &Path) {
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_civil_utc() {
        assert_eq!(utc_timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(utc_timestamp(1_792_675_392), "2026-10-22 13:23:12 UTC");
    }

    #[test]
    fn backtraces_start_at_the_panicking_code() {
        let backtrace = "   0: std::backtrace::Backtrace::create\n             at backtrace.rs:331\n   \
                         1: std::panicking::panic_handler\n   2: core::panicking::panic_fmt\n             \
                         at panicking.rs:80\n   3: inkpainter3d::main\n   4: std::rt::lang_start";
        assert_eq!(
            trim_backtrace(backtrace),
            "   3: inkpainter3d::main\n   4: std::rt::lang_start"
        );
        assert_eq!(trim_backtrace("   0: a::b"), "   0: a::b");
        // `catch_unwind` in the runtime's start comes after the code and stays.
        let late =
            "   0: core::panicking::panic_fmt\n   1: app::f\n   2: std::panicking::catch_unwind";
        assert_eq!(
            trim_backtrace(late),
            "   1: app::f\n   2: std::panicking::catch_unwind"
        );
    }

    #[test]
    fn encoding_keeps_only_unreserved_characters() {
        assert_eq!(encode("a b&c=ä\n"), "a%20b%26c%3D%C3%A4%0A");
    }

    #[test]
    fn long_reports_are_cut_to_fit_the_url() {
        let log: String = (0..2000)
            .map(|i| format!("line {i} of the log\n"))
            .collect();
        let url = issue_url(
            "Title",
            &[
                Part::code("Version and system", "Version: 0.1.0"),
                Part::log(&log),
            ],
        );
        assert!(url.len() <= MAX_URL);
        assert!(url.contains("line%201999"), "the newest log lines stay");
        assert!(!url.contains("line%200%20"), "the oldest go");
        assert!(url.contains("Version%3A%200.1.0"));
    }

    #[test]
    fn bug_reports_carry_the_description_in_the_body() {
        let url = bug_issue_url("Gaps", "The brush leaves gaps.", false);
        assert!(url.starts_with(&format!("{NEW_ISSUE}?labels=bug&title=Gaps&body=")));
        assert!(url.contains(&encode("### What happened?\n\nThe brush leaves gaps.")));
        assert!(url.contains(&encode("### Version and system\n\n```text\nVersion: ")));
        assert!(!url.contains(&encode("### Log")), "no log without the tick");
    }

    #[test]
    fn crash_sections_are_found() {
        let report = CrashReport {
            path: PathBuf::new(),
            text: format!(
                "{CRASH_HEADER}\nTime: x\n\n{SECTION}Panic\nboom\nat a.rs:1:2 (thread 'main')\n\n\
                 {SECTION}System\nVersion: 1\n\n{SECTION}Backtrace\n0: f\n\n{SECTION}Log\nhello\n"
            ),
        };
        assert_eq!(report.message(), "boom");
        assert_eq!(report.section("System"), "Version: 1");
        assert_eq!(report.section("Log"), "hello");
        assert!(report.issue_url().contains("title=Crash%3A%20boom"));
    }
}
