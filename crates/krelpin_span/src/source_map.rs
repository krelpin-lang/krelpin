use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use crate::{BytePos, Span};

#[derive(Debug)]
pub struct SourceFile {
    pub name: PathBuf,
    pub src: String,
    pub start_pos: BytePos,
    line_starts: Vec<BytePos>,
}

impl SourceFile {
    fn new(name: PathBuf, src: String, start_pos: BytePos) -> SourceFile {
        let line_starts = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(index, _)| index + 1))
            .map(|offset| start_pos + offset as u32)
            .collect();

        SourceFile {
            name,
            src,
            start_pos,
            line_starts,
        }
    }

    pub fn end_pos(&self) -> BytePos {
        self.start_pos + self.src.len() as u32
    }

    pub fn contains(&self, pos: BytePos) -> bool {
        self.start_pos <= pos && pos <= self.end_pos()
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn lookup_line(&self, pos: BytePos) -> usize {
        self.line_starts.partition_point(|&start| start <= pos) - 1
    }

    pub fn line(&self, index: usize) -> &str {
        let lo = self.offset(self.line_starts[index]);
        let hi = self
            .line_starts
            .get(index + 1)
            .map_or(self.src.len(), |&next| self.offset(next));
        self.src[lo..hi].trim_end_matches(['\n', '\r'])
    }

    pub fn text(&self, span: Span) -> &str {
        &self.src[self.offset(span.lo)..self.offset(span.hi)]
    }

    fn lookup_line_col(&self, pos: BytePos) -> (usize, usize) {
        let line = self.lookup_line(pos);
        let line_start = self.offset(self.line_starts[line]);
        let col = self.src[line_start..self.offset(pos)].chars().count();
        (line, col)
    }

    fn offset(&self, pos: BytePos) -> usize {
        (pos - self.start_pos) as usize
    }
}

#[derive(Clone, Debug)]
pub struct Loc {
    pub file: Arc<SourceFile>,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Default)]
pub struct SourceMap {
    files: RwLock<Vec<Arc<SourceFile>>>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    pub fn load_file(&self, path: &Path) -> io::Result<Arc<SourceFile>> {
        let src = fs::read_to_string(path)?;
        Ok(self.new_source_file(path, src))
    }

    pub fn new_source_file(&self, name: impl Into<PathBuf>, src: String) -> Arc<SourceFile> {
        let mut files = self.files.write().unwrap_or_else(PoisonError::into_inner);
        let start_pos = files.last().map_or(BytePos(0), |file| file.end_pos() + 1);
        let file = Arc::new(SourceFile::new(name.into(), src, start_pos));
        files.push(Arc::clone(&file));
        file
    }

    pub fn lookup_file(&self, pos: BytePos) -> Option<Arc<SourceFile>> {
        let files = self.files.read().unwrap_or_else(PoisonError::into_inner);
        let index = files
            .partition_point(|file| file.start_pos <= pos)
            .checked_sub(1)?;
        let file = &files[index];
        file.contains(pos).then(|| Arc::clone(file))
    }

    pub fn lookup_char_pos(&self, pos: BytePos) -> Option<Loc> {
        let file = self.lookup_file(pos)?;
        let (line, col) = file.lookup_line_col(pos);
        Some(Loc {
            file,
            line: line + 1,
            col: col + 1,
        })
    }

    pub fn span_to_snippet(&self, span: Span) -> Option<String> {
        let file = self.lookup_file(span.lo)?;
        file.contains(span.hi).then(|| file.text(span).to_owned())
    }
}
