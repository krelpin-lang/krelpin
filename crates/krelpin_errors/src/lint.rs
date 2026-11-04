use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LintLevel {
    Allow,
    Warn,
    Deny,
    Forbid,
}

impl LintLevel {
    pub fn flag(self) -> &'static str {
        match self {
            LintLevel::Allow => "-A",
            LintLevel::Warn => "-W",
            LintLevel::Deny => "-D",
            LintLevel::Forbid => "-F",
        }
    }
}

pub struct Lint {
    pub name: &'static str,
    pub summary: &'static str,
    pub explanation: &'static str,
}

pub const WARNINGS: &str = "warnings";

pub const LINTS: &[Lint] = &[
    Lint {
        name: "unreachable_patterns",
        summary: "a `case` that can never match",
        explanation: "A `case` is unreachable when the cases above it already match every \
                      value it could match. The case is dead code, which usually means the \
                      cases are in the wrong order or a pattern is broader than intended.",
    },
    Lint {
        name: "unnecessary_safe_call",
        summary: "`?.` on a value that is never null",
        explanation: "`a?.b` reads `b` only when `a` is not null. When the type of `a` cannot \
                      be null the check never fails, so `a.b` says the same thing more \
                      plainly.",
    },
    Lint {
        name: "unnecessary_elvis",
        summary: "`?:` whose left side is never null",
        explanation: "`a ?: b` evaluates to `b` only when `a` is null. When the type of `a` \
                      cannot be null the default is never used and can be removed.",
    },
    Lint {
        name: "unexpected_cfgs",
        summary: "a `@Cfg` condition that `--check-cfg` does not expect",
        explanation: "When `--check-cfg` is given, every name and value used in `@Cfg` must \
                      be declared by it. An unexpected name or value is usually a typo that \
                      would silently remove code from the program.",
    },
    Lint {
        name: "unknown_lints",
        summary: "a lint flag that names no lint",
        explanation: "A lint given to `-A`, `-W`, `-D`, `-F` or `--force-warn` must be one of \
                      the lints listed by `krelpinc --explain lints`, or `warnings` for all of \
                      them.",
    },
];

pub fn lint(name: &str) -> Option<&'static Lint> {
    let name = normalize(name);
    LINTS.iter().find(|lint| lint.name == name)
}

pub fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

#[derive(Clone, Debug, Default)]
pub struct LintLevels {
    levels: HashMap<String, LintLevel>,
    forced: Vec<String>,
    cap: Option<LintLevel>,
}

impl LintLevels {
    pub fn set(&mut self, name: &str, level: LintLevel) {
        let name = normalize(name);
        let entry = self.levels.entry(name).or_insert(level);
        if *entry != LintLevel::Forbid {
            *entry = level;
        }
    }

    pub fn force_warn(&mut self, name: &str) {
        self.forced.push(normalize(name));
    }

    pub fn cap(&mut self, level: LintLevel) {
        self.cap = Some(level);
    }

    pub fn level(&self, name: &str) -> (LintLevel, Option<String>) {
        let specific = self.levels.get(name).map(|level| (*level, name.to_owned()));
        let group = self
            .levels
            .get(WARNINGS)
            .map(|level| (*level, WARNINGS.to_owned()));
        let (mut level, mut source) = match (specific, group) {
            (Some((LintLevel::Forbid, name)), _) => (LintLevel::Forbid, Some(name)),
            (_, Some((LintLevel::Forbid, group))) => (LintLevel::Forbid, Some(group)),
            (Some((level, name)), _) => (level, Some(name)),
            (None, Some((level, group))) => (level, Some(group)),
            (None, None) => (LintLevel::Warn, None),
        };
        let forced = self
            .forced
            .iter()
            .any(|forced| forced == name || forced == WARNINGS);
        if forced && level == LintLevel::Allow {
            level = LintLevel::Warn;
            source = Some(format!("--force-warn {name}"));
        }
        if let Some(cap) = self.cap
            && level > cap
            && !forced
        {
            level = cap;
        }
        (level, source)
    }
}
