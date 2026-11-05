use krelpin_ast::ast::{
    CompilationUnit, ItemKind, MacroBody, MacroDef, MemberKind, Param, TyKind, is_intrinsic,
};
use krelpin_ast::package::PackagePath;
use krelpin_ast::token::TokenKind;
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_parse::{lexer, parser};
use krelpin_span::{Expansion, SourceMap, Symbol};

use crate::proc_macro::escape_literal;

const SERVER_CLASS: &str = "macros#";

pub(crate) fn define_macros(
    units: &mut [CompilationUnit],
    source_map: &SourceMap,
    dcx: &DiagCtxt,
    proc_macro: Option<&str>,
) {
    let user_units = units
        .iter()
        .filter(|unit| !PackagePath::of_unit(unit).is_std())
        .count();
    let mut seen = 0;
    let mut defs: Vec<MacroDef> = Vec::new();
    let mut root = None;
    for (index, unit) in units.iter_mut().enumerate() {
        if PackagePath::of_unit(unit).is_std() {
            continue;
        }
        seen += 1;
        let macros = std::mem::take(&mut unit.macros);
        if proc_macro.is_none() {
            for def in &macros {
                reject_definition(def, dcx);
            }
            continue;
        }
        if seen < user_units {
            for def in &macros {
                dcx.emit(
                    Diagnostic::error("macros must be defined in the root of a `proc-macro` crate")
                        .with_span(def.name.span),
                );
            }
            continue;
        }
        root = Some(index);
        defs = macros;
    }
    let (Some(crate_name), Some(root)) = (proc_macro, root) else {
        return;
    };
    let unit = &mut units[root];
    reject_main(unit, dcx);
    let package = PackagePath::of_unit(unit);
    let mut macros = Vec::new();
    for def in defs {
        if let Some(body) = checked_body(&def, dcx) {
            let qualified = if package == PackagePath::default() {
                def.name.name.as_str().to_owned()
            } else {
                format!("{package}.{}", def.name.name)
            };
            macros.push((qualified, def.name, body));
        }
    }

    let mut src = String::from(
        "final class MacroServer {\n    static void main() {\n        \
         std.macros.Bridge bridge = new std.macros.Bridge();\n",
    );
    for (qualified, _, _) in &macros {
        src.push_str(&format!(
            "        bridge.declare(\"{}\");\n",
            escape_literal(qualified)
        ));
    }
    src.push_str(&format!(
        "        bridge.ready(\"{}\");\n        while (bridge.next()) {{\n            \
         std.macros.TokenStream args = bridge.read();\n            \
         std.macros.TokenStream item = bridge.read();\n            try {{\n",
        escape_literal(crate_name)
    ));
    let mut dispatch = String::from("                ");
    for (qualified, name, _) in &macros {
        dispatch.push_str(&format!(
            "if (bridge.calls(\"{}\")) {{\n                    \
             bridge.reply({}(args, item));\n                }} else ",
            escape_literal(qualified),
            name.name
        ));
    }
    dispatch.push_str("{\n                    bridge.unknown();\n                }\n");
    src.push_str(&dispatch);
    src.push_str(
        "            } catch (std.macros.MacroError error) {\n                \
         bridge.fail(&error);\n            }\n        }\n    }\n",
    );
    for (_, name, _) in &macros {
        src.push_str(&format!(
            "\n    static std.macros.TokenStream {}(std.macros.TokenStream args, \
             std.macros.TokenStream item) {{}}\n",
            name.name
        ));
    }
    src.push_str("}\n");

    let expansion = Expansion {
        call_site: unit.span.shrink_to_lo(),
        macro_name: "proc-macro".to_owned(),
    };
    let file =
        source_map.new_expansion_file(format!("<macro server of {crate_name}>"), src, expansion);
    let tokens = lexer::lex(&file, dcx);
    let class = Symbol::intern("MacroServer");
    let tokens = tokens
        .into_iter()
        .map(|mut token| {
            if token.kind == TokenKind::Ident(class) {
                token.kind = TokenKind::Ident(Symbol::intern(SERVER_CLASS));
            }
            token
        })
        .collect();
    let mut items = parser::parse_items(tokens, dcx);
    let Some(mut server) = items.pop() else {
        return;
    };
    if let ItemKind::Class(class) = &mut server.kind {
        let mut bodies = macros.into_iter();
        for member in class.members.iter_mut().skip(1) {
            let (Some((_, name, body)), MemberKind::Method(method)) =
                (bodies.next(), &mut member.kind)
            else {
                continue;
            };
            method.name = name;
            method.params = body.params;
            method.body = Some(body.body);
            member.span = body.span;
        }
    }
    unit.items.push(server);
}

fn reject_definition(def: &MacroDef, dcx: &DiagCtxt) {
    let diagnostic = if def.body.is_some() {
        Diagnostic::error("macros can only be defined in a `proc-macro` crate")
            .with_span(def.name.span)
            .with_help(
                "build this file with `--crate-type proc-macro`, then use its macros from \
                 other crates with `--extern`",
            )
    } else {
        Diagnostic::error("only the standard library can declare macros without a body")
            .with_span(def.name.span)
            .with_help("write `macro Name(TokenStream args, TokenStream item) { ... }`")
    };
    dcx.emit(diagnostic);
}

fn reject_main(unit: &CompilationUnit, dcx: &DiagCtxt) {
    for item in &unit.items {
        let members = match &item.kind {
            ItemKind::Class(class) => &class.members,
            ItemKind::Record(def) => &def.members,
            ItemKind::Interface(interface) => &interface.members,
            ItemKind::Enum(def) => &def.members,
        };
        for member in members {
            if let MemberKind::Method(method) = &member.kind
                && method.receiver.is_none()
                && method.name.name.as_str() == "main"
            {
                dcx.emit(
                    Diagnostic::error("a `proc-macro` crate cannot have a `main` method")
                        .with_span(method.name.span)
                        .with_note("the compiler writes the entry point of a macro crate"),
                );
            }
        }
    }
}

fn checked_body(def: &MacroDef, dcx: &DiagCtxt) -> Option<MacroBody> {
    let name = def.name;
    if is_intrinsic(&def.annotations) {
        dcx.emit(
            Diagnostic::error("only the standard library has intrinsic macros")
                .with_span(name.span),
        );
        return None;
    }
    let Some(body) = &def.body else {
        dcx.emit(
            Diagnostic::error(format!("the macro `{}` has no body", name.name))
                .with_span(name.span)
                .with_help(format!(
                    "write `macro {}(TokenStream args, TokenStream item) {{ ... }}`",
                    name.name
                )),
        );
        return None;
    };
    if matches!(name.name.as_str(), "main" | "MacroServer") {
        dcx.emit(
            Diagnostic::error(format!("a macro cannot be named `{}`", name.name))
                .with_span(name.span),
        );
        return None;
    }
    if body.params.len() != 2 {
        dcx.emit(
            Diagnostic::error(format!(
                "the macro `{}` takes {} parameter{}, but macros take two",
                name.name,
                body.params.len(),
                if body.params.len() == 1 { "" } else { "s" }
            ))
            .with_span(name.span)
            .with_note(
                "the first holds the arguments of the annotation, the second the code it is on",
            )
            .with_help(format!(
                "write `macro {}(TokenStream args, TokenStream item)`",
                name.name
            )),
        );
        return None;
    }
    let mut valid = true;
    for param in &body.params {
        if !is_token_stream(param) {
            dcx.emit(
                Diagnostic::error("macro parameters are `TokenStream`s")
                    .with_span(param.ty.span)
                    .with_help("import `std.macros.TokenStream` and use it here"),
            );
            valid = false;
        }
    }
    valid.then(|| body.clone())
}

fn is_token_stream(param: &Param) -> bool {
    if param.variadic {
        return false;
    }
    let TyKind::Path { path, args } = &param.ty.kind else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    let names: Vec<&str> = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect();
    matches!(
        names.as_slice(),
        ["TokenStream"] | ["std", "macros", "TokenStream"]
    )
}
