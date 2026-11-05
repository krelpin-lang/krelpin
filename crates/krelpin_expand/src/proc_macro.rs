use std::cell::RefCell;
use std::fmt;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const HELLO: &str = "krelpin-macro-server";
const PROTOCOL: &str = "1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireKind {
    Ident,
    Literal,
    Punct,
    Doc,
}

impl WireKind {
    fn name(self) -> &'static str {
        match self {
            WireKind::Ident => "ident",
            WireKind::Literal => "literal",
            WireKind::Punct => "punct",
            WireKind::Doc => "doc",
        }
    }

    fn parse(name: &str) -> Option<WireKind> {
        match name {
            "ident" => Some(WireKind::Ident),
            "literal" => Some(WireKind::Literal),
            "punct" => Some(WireKind::Punct),
            "doc" => Some(WireKind::Doc),
            _ => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            WireKind::Ident => "an identifier",
            WireKind::Literal => "a literal",
            WireKind::Punct => "punctuation",
            WireKind::Doc => "a doc comment",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireToken {
    pub kind: WireKind,
    pub span: Option<usize>,
    pub text: String,
}

impl WireToken {
    pub(crate) fn encode(&self) -> String {
        let span = self.span.map_or(-1, |span| span as i64);
        format!("{}\t{span}\t{}", self.kind.name(), escape(&self.text))
    }

    pub(crate) fn decode(line: &str) -> Option<WireToken> {
        let mut fields = line.splitn(3, '\t');
        let kind = WireKind::parse(fields.next()?)?;
        let span: i64 = fields.next()?.parse().ok()?;
        let text = unescape(fields.next()?);
        Some(WireToken {
            kind,
            span: usize::try_from(span).ok(),
            text,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CallError {
    Reported {
        message: String,
        span: Option<usize>,
    },
    Panicked(String),
    Failed(String),
}

pub struct ProcMacroCrate {
    name: String,
    path: PathBuf,
    macros: Vec<String>,
    server: RefCell<Option<Server>>,
}

impl fmt::Debug for ProcMacroCrate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcMacroCrate")
            .field("name", &self.name)
            .field("path", &self.path)
            .field("macros", &self.macros)
            .finish()
    }
}

struct Server {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    output: BufReader<ChildStdout>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.input = None;
        let _ = self.child.wait();
    }
}

impl ProcMacroCrate {
    pub fn load(name: &str, path: &Path) -> Result<ProcMacroCrate, String> {
        let path = std::path::absolute(path)
            .ok()
            .filter(|path| path.is_file())
            .ok_or_else(|| {
                format!(
                    "couldn't find the macro crate `{name}` at `{}`",
                    path.display()
                )
            })?;
        let path = path.as_path();
        let invalid = || {
            format!(
                "`{}` is not a macro crate; build it with `--crate-type proc-macro`",
                path.display()
            )
        };
        let incompatible = || {
            format!(
                "`{}` was built by an incompatible compiler; rebuild it",
                path.display()
            )
        };
        let bytes = std::fs::read(path)
            .map_err(|error| format!("couldn't read `{}`: {error}", path.display()))?;
        let marker = format!("{HELLO}\t{PROTOCOL}\t");
        if !contains(&bytes, marker.as_bytes()) {
            return Err(if contains(&bytes, HELLO.as_bytes()) {
                incompatible()
            } else {
                invalid()
            });
        }
        let (server, hello) = Server::spawn(path)?;
        let mut fields = hello.split('\t');
        if fields.next() != Some(HELLO) {
            return Err(invalid());
        }
        if fields.next() != Some(PROTOCOL) {
            return Err(incompatible());
        }
        let crate_name = unescape(fields.next().ok_or_else(invalid)?);
        if crate_name != name {
            return Err(format!(
                "`{}` is the macro crate `{crate_name}`, not `{name}`",
                path.display()
            ));
        }
        let macros = fields.map(unescape).collect();
        Ok(ProcMacroCrate {
            name: name.to_owned(),
            path: path.to_owned(),
            macros,
            server: RefCell::new(Some(server)),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn macros(&self) -> &[String] {
        &self.macros
    }

    pub fn call(
        &self,
        name: &str,
        args: &[WireToken],
        item: &[WireToken],
        lex: &mut dyn FnMut(&str) -> Vec<WireToken>,
    ) -> Result<Vec<WireToken>, CallError> {
        let mut slot = self.server.borrow_mut();
        if slot.is_none() {
            let (server, _) = Server::spawn(&self.path).map_err(CallError::Failed)?;
            *slot = Some(server);
        }
        let server = slot.as_mut().expect("the macro server is running");
        let result = server.call(name, args, item, lex);
        if matches!(result, Err(CallError::Panicked(_) | CallError::Failed(_))) {
            *slot = None;
        }
        result
    }
}

impl Server {
    fn spawn(path: &Path) -> Result<(Server, String), String> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("couldn't run `{}`: {error}", path.display()))?;
        let input = child.stdin.take().map(BufWriter::new);
        let output = child.stdout.take().map(BufReader::new);
        let Some(output) = output else {
            return Err(format!("couldn't talk to `{}`", path.display()));
        };
        let mut server = Server {
            child,
            input,
            output,
        };
        match server.read_line() {
            Ok(Some(hello)) => Ok((server, hello)),
            _ => Err(format!(
                "`{}` is not a macro crate; build it with `--crate-type proc-macro`",
                path.display()
            )),
        }
    }

    fn call(
        &mut self,
        name: &str,
        args: &[WireToken],
        item: &[WireToken],
        lex: &mut dyn FnMut(&str) -> Vec<WireToken>,
    ) -> Result<Vec<WireToken>, CallError> {
        let mut request = format!("expand\t{}\n", escape(name));
        push_stream(&mut request, args);
        push_stream(&mut request, item);
        self.write(&request).map_err(|error| self.failure(&error))?;
        loop {
            let line = self.expect_line()?;
            let (tag, rest) = line.split_once('\t').unwrap_or((line.as_str(), ""));
            match tag {
                "lex" => {
                    let tokens = lex(&unescape(rest));
                    let mut reply = String::new();
                    push_stream(&mut reply, &tokens);
                    self.write(&reply).map_err(|error| self.failure(&error))?;
                }
                "ok" => {
                    let count: usize = rest
                        .parse()
                        .map_err(|_| self.malformed(&format!("a bad token count `{rest}`")))?;
                    let mut tokens = Vec::with_capacity(count);
                    for _ in 0..count {
                        let line = self.expect_line()?;
                        let token = WireToken::decode(&line)
                            .ok_or_else(|| self.malformed(&format!("a bad token `{line}`")))?;
                        tokens.push(token);
                    }
                    return Ok(tokens);
                }
                "error" => {
                    let (span, message) = rest.split_once('\t').unwrap_or(("-1", rest));
                    let span = span
                        .parse::<i64>()
                        .ok()
                        .and_then(|span| span.try_into().ok());
                    return Err(CallError::Reported {
                        message: unescape(message),
                        span,
                    });
                }
                "panic" => return Err(CallError::Panicked(unescape(rest))),
                _ => return Err(self.malformed(&format!("an unknown message `{tag}`"))),
            }
        }
    }

    fn write(&mut self, text: &str) -> io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        input.write_all(text.as_bytes())?;
        input.flush()
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        if self.output.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if line.ends_with('\n') {
            line.pop();
        }
        Ok(Some(line))
    }

    fn expect_line(&mut self) -> Result<String, CallError> {
        match self.read_line() {
            Ok(Some(line)) => Ok(line),
            Ok(None) => Err(self.exited()),
            Err(error) => Err(self.failure(&error)),
        }
    }

    fn exited(&mut self) -> CallError {
        self.input = None;
        match self.child.wait() {
            Ok(status) => CallError::Failed(format!("the macro server stopped ({status})")),
            Err(error) => CallError::Failed(format!("the macro server stopped: {error}")),
        }
    }

    fn failure(&mut self, error: &io::Error) -> CallError {
        match self.exited() {
            CallError::Failed(message) => CallError::Failed(format!("{message}: {error}")),
            other => other,
        }
    }

    fn malformed(&self, what: &str) -> CallError {
        CallError::Failed(format!("the macro server sent {what}"))
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn push_stream(out: &mut String, tokens: &[WireToken]) {
    out.push_str(&format!("tokens\t{}\n", tokens.len()));
    for token in tokens {
        out.push_str(&token.encode());
        out.push('\n');
    }
}

pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

pub(crate) fn escape_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}
