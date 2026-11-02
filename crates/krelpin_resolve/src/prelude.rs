use crate::PackagePath;

const PACKAGES: &[&str] = &["std.lang", "std.util", "std.sync"];

pub(crate) fn packages() -> Vec<PackagePath> {
    PACKAGES
        .iter()
        .map(|package| PackagePath::parse(package))
        .collect()
}
