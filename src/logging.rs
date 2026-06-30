use std::io::Write;
use tracing::field::Visit;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FormatEvent, FormatFields};

const BANNER: &str = "\x1b[36m\
__________                __  .____    .__        __    \n\
\\______   \\__ __  _______/  |_|    |   |__| ____ |  | __\n\
 |       _/  |  \\/  ___/\\   __\\    |   |  |/    \\|  |/ /\n\
 |    |   \\  |  /\\___ \\  |  | |    |___|  |   |  \\    < \n\
 |____|_  /____//____  > |__| |_______ \\__|___|  /__|_ \\\n\
        \\/           \\/               \\/       \\/     \\/\n\
\x1b[0m\x1b[33mRustLink v{}\x1b[0m\n\
\x1b[90mMade by Kurenai\x1b[0m\n";

pub fn started(target: &str, message: String) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(
        stdout,
        "[{}] \x1b[1m\x1b[3;44m[STARTED] >\x1b[0m: {} > {}",
        time_prefix(),
        target,
        message
    );
    let _ = stdout.flush();
}

pub fn mem_trace(message: String) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "\x1b[35m[MEM]\x1b[0m {}", message);
    let _ = stdout.flush();
}

pub fn print_banner(version: &str) {
    let banner = BANNER.replace("{}", version);
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(banner.as_bytes());
    let _ = stdout.flush();
}

fn time_prefix() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_ms = now.as_millis();
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let secs = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{secs:02}.{ms:03}")
}

fn level_style(level: &tracing::Level) -> &'static str {
    match *level {
        tracing::Level::ERROR => "\x1b[1m\x1b[3;41m",
        tracing::Level::WARN => "\x1b[1m\x1b[3;43m",
        tracing::Level::INFO => "\x1b[1m\x1b[3;42m",
        tracing::Level::DEBUG => "\x1b[1m\x1b[3;45m",
        tracing::Level::TRACE => "\x1b[1m\x1b[3;45m",
    }
}

fn level_str(level: &tracing::Level) -> &'static str {
    match *level {
        tracing::Level::ERROR => "ERROR",
        tracing::Level::WARN => "WARN",
        tracing::Level::INFO => "INFO",
        tracing::Level::DEBUG => "DEBUG",
        tracing::Level::TRACE => "TRACE",
    }
}

fn target_name(target: &str) -> &str {
    if target.is_empty() {
        return "";
    }
    target.rsplit("::").next().unwrap_or(target)
}

struct MessageVisitor {
    message: String,
}

impl Default for MessageVisitor {
    fn default() -> Self {
        Self {
            message: String::new(),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let s = format!("{value:?}");
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                self.message = s[1..s.len() - 1].to_string();
            } else {
                self.message = s;
            }
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }
}

struct NodeLinkFormatter;

impl<S, N> FormatEvent<S, N> for NodeLinkFormatter
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let level = event.metadata().level();
        let target = target_name(event.metadata().target());
        let ansi = level_style(level);
        let lvl = level_str(level);
        let reset = "\x1b[0m";
        let time = time_prefix();

        if target.is_empty() {
            writeln!(writer, "[{time}] {ansi}[{lvl}] >{reset} {}", visitor.message)
        } else {
            writeln!(
                writer,
                "[{time}] {ansi}[{lvl}] >{reset}: {target} > {}",
                visitor.message
            )
        }
    }
}

pub fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .event_format(NodeLinkFormatter)
        .try_init();
}
