use std::fmt;

use crate::token::{Lit, LitKind};

use super::{
    Expr, ExprKind, Ident, LambdaBody, LambdaParam, Path, SwitchExprArm, SwitchExprBody, Ty,
    TyKind, UnOp,
};

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} @ {:?}", self.name, self.span)
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{self}\" @ {:?}", self.span)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.segments.iter().map(|s| s.name.as_str()).collect();
        f.write_str(&names.join("."))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TyKind::Primitive(primitive) => f.write_str(primitive.name()),
            TyKind::Path { path, args } => {
                write!(f, "{path}")?;
                if !args.is_empty() {
                    let args: Vec<String> = args.iter().map(Ty::to_string).collect();
                    write!(f, "<{}>", args.join(", "))?;
                }
                Ok(())
            }
            TyKind::Ref {
                mutable,
                sources,
                ty,
            } => {
                f.write_str("&")?;
                if *mutable {
                    f.write_str("mut ")?;
                }
                if !sources.is_empty() {
                    let names: Vec<&str> = sources.iter().map(|s| s.name.as_str()).collect();
                    write!(f, "[{}] ", names.join(", "))?;
                }
                match ty.kind {
                    TyKind::Nullable(_) => write!(f, "({ty})"),
                    _ => write!(f, "{ty}"),
                }
            }
            TyKind::Nullable(ty) => write!(f, "{ty}?"),
            TyKind::Array(ty) => match ty.kind {
                TyKind::Ref { .. } | TyKind::Nullable(_) => write!(f, "({ty})[]"),
                _ => write!(f, "{ty}[]"),
            },
        }
    }
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            LitKind::Str => write!(f, "\"{}\"", self.symbol)?,
            LitKind::Char => write!(f, "'{}'", self.symbol)?,
            LitKind::TextBlock => write!(f, "\"\"\"{}\"\"\"", self.symbol)?,
            LitKind::Integer | LitKind::Float | LitKind::Err => write!(f, "{}", self.symbol)?,
        }
        if let Some(suffix) = self.suffix {
            write!(f, "{suffix}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Lit(lit) => write!(f, "{lit}"),
            ExprKind::Bool(value) => write!(f, "{value}"),
            ExprKind::Null => f.write_str("null"),
            ExprKind::This => f.write_str("this"),
            ExprKind::Super => f.write_str("super"),
            ExprKind::Name(name) => write!(f, "{}", name.name),
            ExprKind::Field { expr, name, safe } => {
                write!(f, "{expr}{}{}", access(*safe), name.name)
            }
            ExprKind::MethodCall {
                receiver,
                name,
                args,
                safe,
            } => {
                if let Some(receiver) = receiver {
                    write!(f, "{receiver}{}", access(*safe))?;
                }
                write!(f, "{}({})", name.name, list(args))
            }
            ExprKind::Call { callee, args } => write!(f, "{callee}({})", list(args)),
            ExprKind::Index { expr, index } => write!(f, "{expr}[{index}]"),
            ExprKind::New { ty, args } => write!(f, "new {ty}({})", list(args)),
            ExprKind::NewArray { elem, len } => write!(f, "new {elem}[{len}]"),
            ExprKind::ArrayInit { ty, elems } => match ty {
                Some(ty) => write!(f, "new {ty} {{{}}}", list(elems)),
                None => write!(f, "{{{}}}", list(elems)),
            },
            ExprKind::Unary { op, expr } => match op {
                UnOp::Neg => write!(f, "(-{expr})"),
                UnOp::Not => write!(f, "(!{expr})"),
                UnOp::BitNot => write!(f, "(~{expr})"),
                UnOp::PreInc => write!(f, "(++{expr})"),
                UnOp::PreDec => write!(f, "(--{expr})"),
                UnOp::PostInc => write!(f, "({expr}++)"),
                UnOp::PostDec => write!(f, "({expr}--)"),
            },
            ExprKind::Borrow { mutable, expr } => {
                let prefix = if *mutable { "&mut " } else { "&" };
                write!(f, "({prefix}{expr})")
            }
            ExprKind::Binary { op, lhs, rhs } => write!(f, "({lhs} {} {rhs})", op.as_str()),
            ExprKind::Assign { op, lhs, rhs } => {
                let op = op.map_or("", |op| op.as_str());
                write!(f, "({lhs} {op}= {rhs})")
            }
            ExprKind::Ternary { cond, then, els } => write!(f, "({cond} ? {then} : {els})"),
            ExprKind::InstanceOf { expr, ty, binding } => match binding {
                Some(binding) => write!(f, "({expr} instanceof {ty} {})", binding.name),
                None => write!(f, "({expr} instanceof {ty})"),
            },
            ExprKind::Cast { ty, expr } => write!(f, "(({ty}) {expr})"),
            ExprKind::Lambda { params, body } => {
                let params: Vec<String> = params.iter().map(LambdaParam::to_string).collect();
                match body.as_ref() {
                    LambdaBody::Expr(expr) => write!(f, "(({}) -> {expr})", params.join(", ")),
                    LambdaBody::Block(_) => write!(f, "(({}) -> {{ ... }})", params.join(", ")),
                }
            }
            ExprKind::MethodRef { expr, name } => write!(f, "{expr}::{}", name.name),
            ExprKind::Switch { scrutinee, arms } => {
                let arms: Vec<String> = arms.iter().map(SwitchExprArm::to_string).collect();
                write!(f, "switch ({scrutinee}) {{ {} }}", arms.join(" "))
            }
        }
    }
}

impl fmt::Display for SwitchExprArm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.labels {
            Some(labels) => write!(f, "case {} -> ", list(labels))?,
            None => f.write_str("default -> ")?,
        }
        match &self.body {
            SwitchExprBody::Expr(expr) => write!(f, "{expr};"),
            SwitchExprBody::Block(_) => f.write_str("{ ... }"),
            SwitchExprBody::Throw(expr) => write!(f, "throw {expr};"),
        }
    }
}

impl fmt::Display for LambdaParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.ty {
            Some(ty) => write!(f, "{ty} {}", self.name.name),
            None => write!(f, "{}", self.name.name),
        }
    }
}

fn access(safe: bool) -> &'static str {
    if safe { "?." } else { "." }
}

fn list(exprs: &[Expr]) -> String {
    let parts: Vec<String> = exprs.iter().map(Expr::to_string).collect();
    parts.join(", ")
}
