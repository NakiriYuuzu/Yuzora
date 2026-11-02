//! SQL text helpers: splitting a script into statements, and what a
//! statement is when it comes to row counts and the sidebar's order.

use std::ops::Range;

use crate::config::Engine;
use crate::model::{Cell, DbObject, ObjectKind};

/// One statement of a script: its text (trimmed, without the separator) and
/// where it sits in the source (byte range).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub text: String,
    pub range: Range<usize>,
}

/// Splits `sql` into statements for `engine`.
///
/// PostgreSQL and SQLite end a statement at `;`, SQL Server only at a line
/// holding nothing but `GO`. Comments, quoted text and identifiers (and for
/// PostgreSQL dollar quotes) never end one. A stretch of only comments and
/// blanks is not a statement; a comment ahead of a statement belongs to it.
pub fn split(sql: &str, engine: Engine) -> Vec<Statement> {
    split_with(sql, engine, false)
}

/// [`split`], for a PostgreSQL session where `\` escapes in ordinary
/// strings too (`standard_conforming_strings = off`) when `backslash`.
pub fn split_with(sql: &str, engine: Engine, backslash: bool) -> Vec<Statement> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut seg = Segment::default();
    let mut line_start = true;
    let mut i = 0;
    while i < b.len() {
        if line_start {
            line_start = false;
            if engine == Engine::Mssql {
                if let Some(end) = go_line(b, i) {
                    flush(sql, start..i, &mut seg, &mut out);
                    start = end;
                    i = end;
                    continue;
                }
            }
        }
        let c = b[i];
        match c {
            b'\n' => {
                line_start = true;
                i += 1;
            }
            _ if c.is_ascii_whitespace() => i += 1,
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => i = skip_block_comment(b, i, engine != Engine::Sqlite),
            b'\'' => {
                seg.has_content = true;
                let escapes = engine == Engine::Postgres
                    && (backslash
                        || i > 0 && matches!(b[i - 1], b'e' | b'E') && (i < 2 || !is_word_byte(b[i - 2], engine)));
                i = skip_quoted(b, i, b'\'', escapes);
            }
            b'"' => {
                seg.has_content = true;
                i = skip_quoted(b, i, b'"', false);
            }
            b'`' if engine == Engine::Sqlite => {
                seg.has_content = true;
                i = skip_quoted(b, i, b'`', false);
            }
            b'[' if engine != Engine::Postgres => {
                seg.has_content = true;
                i = skip_bracketed(b, i, engine == Engine::Mssql);
            }
            b'$' if engine == Engine::Postgres => {
                seg.has_content = true;
                i = skip_dollar_quoted(b, i).unwrap_or(i + 1);
            }
            b';' if engine != Engine::Mssql => {
                if seg.in_body {
                    seg.has_content = true;
                    i += 1;
                } else {
                    flush(sql, start..i, &mut seg, &mut out);
                    i += 1;
                    start = i;
                }
            }
            _ if is_word_byte(c, engine) => {
                let from = i;
                while i < b.len() && is_word_byte(b[i], engine) {
                    i += 1;
                }
                seg.has_content = true;
                seg.word(&b[from..i], engine);
            }
            _ => {
                seg.has_content = true;
                i += 1;
            }
        }
    }
    flush(sql, start..b.len(), &mut seg, &mut out);
    out
}

/// The statement under `offset` (between two, the one before; ahead of the
/// first, the first).
pub fn statement_at(sql: &str, offset: usize, engine: Engine) -> Option<Statement> {
    statement_at_with(sql, offset, engine, false)
}

/// [`statement_at`], to run on its own from the editor's cursor: its text
/// (empty when there is none). `None` when PostgreSQL could end it in two
/// places, by the session's `standard_conforming_strings` the editor does
/// not know.
pub fn runnable_statement_at(sql: &str, offset: usize, engine: Engine) -> Option<String> {
    let statement = statement_at(sql, offset, engine);
    if engine == Engine::Postgres && sql.contains('\\') && statement_at_with(sql, offset, engine, true) != statement {
        return None;
    }
    Some(statement.map(|s| s.text).unwrap_or_default())
}

fn statement_at_with(sql: &str, offset: usize, engine: Engine, backslash: bool) -> Option<Statement> {
    let all = split_with(sql, engine, backslash);
    all.iter()
        .find(|s| s.range.contains(&offset))
        .or_else(|| all.iter().rev().find(|s| s.range.end <= offset))
        .or_else(|| all.first())
        .cloned()
}

/// Whether the statement is one that reports how many rows it changed
/// (`INSERT`, `UPDATE`, `DELETE`…), judged by its first word. A `CREATE
/// TABLE` or `SET` has no such count, whatever the driver says.
pub fn changes_rows(statement: &str) -> bool {
    matches!(
        first_word(statement).as_str(),
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "COPY" | "WITH" | "FETCH" | "MOVE" | "EXECUTE" | "EXEC"
    )
}

/// Whether a PostgreSQL statement may change how the session reads `\` in
/// ordinary strings: one naming `standard_conforming_strings`, or one that
/// takes settings back (`RESET`, `DISCARD`, and ending a transaction, which
/// undoes a `SET LOCAL` or a rolled back `SET`).
pub fn may_change_string_escapes(statement: &str) -> bool {
    statement.to_ascii_lowercase().contains("standard_conforming_strings")
        || matches!(first_word(statement).as_str(), "RESET" | "DISCARD" | "ROLLBACK" | "ABORT" | "COMMIT" | "END")
}

/// The statement's first word, upper case, past leading comments.
fn first_word(statement: &str) -> String {
    let b = statement.as_bytes();
    let mut i = 0;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b[i..].starts_with(b"--") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            i = skip_block_comment(b, i, true);
        } else {
            break;
        }
    }
    let word = b[i..].iter().take_while(|c| c.is_ascii_alphabetic()).count();
    statement[i..i + word].to_ascii_uppercase()
}

/// The sidebar's order: schema, then tables before views before routines…, then name.
pub fn sort_objects(objects: &mut [DbObject]) {
    fn rank(kind: ObjectKind) -> u8 {
        match kind {
            ObjectKind::Table => 0,
            ObjectKind::View => 1,
            ObjectKind::MaterializedView => 2,
            ObjectKind::Sequence => 3,
            ObjectKind::Function => 4,
            ObjectKind::Procedure => 5,
            ObjectKind::Trigger => 6,
            ObjectKind::Index => 7,
            ObjectKind::Type => 8,
            ObjectKind::Other => 9,
        }
    }
    objects.sort_by(|a, b| (&a.schema, rank(a.kind), &a.name).cmp(&(&b.schema, rank(b.kind), &b.name)));
}

/// Why a write-back stopped: the row with this key was found `matched` times,
/// not once, so nothing was written.
pub fn key_mismatch(key: &[(String, Cell)], matched: u64) -> String {
    let key: Vec<String> = key.iter().map(|(column, value)| format!("{column} = {}", value.display())).collect();
    crate::i18n::t("engine.key_mismatch", &[("key", key.join(", ")), ("count", matched.to_string())])
}

/// Write-back refused: the connection is inside a transaction the SQL tab
/// left open, which the write-back's own commit would get tangled in.
pub fn open_transaction() -> crate::error::DbError {
    crate::error::DbError::NotEditable(crate::i18n::t("edit.open_transaction", &[]))
}

// ---------------------------------------------------------------- scanning

/// What the statement being read so far is about: whether anything but
/// comments is in it, and whether a `BEGIN … END` body is open (a trigger's, a
/// function's) in which `;` does not end it.
#[derive(Default)]
struct Segment {
    has_content: bool,
    /// The first words, to see `CREATE [TEMP] TRIGGER`.
    head: Vec<Word>,
    previous: Option<Word>,
    in_body: bool,
    case_depth: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Word {
    Create,
    Temp,
    Trigger,
    Begin,
    Atomic,
    Case,
    End,
    Other,
}

impl Word {
    fn of(text: &[u8]) -> Word {
        const WORDS: [(&str, Word); 8] = [
            ("create", Word::Create),
            ("temp", Word::Temp),
            ("temporary", Word::Temp),
            ("trigger", Word::Trigger),
            ("begin", Word::Begin),
            ("atomic", Word::Atomic),
            ("case", Word::Case),
            ("end", Word::End),
        ];
        WORDS.iter().find(|(w, _)| text.eq_ignore_ascii_case(w.as_bytes())).map_or(Word::Other, |(_, k)| *k)
    }
}

impl Segment {
    fn word(&mut self, text: &[u8], engine: Engine) {
        let word = Word::of(text);
        if self.head.len() < 3 {
            self.head.push(word);
        }
        match word {
            Word::Case => self.case_depth += 1,
            Word::End if self.case_depth > 0 => self.case_depth -= 1,
            Word::End => self.in_body = false,
            Word::Begin if engine == Engine::Sqlite && self.is_create_trigger() => self.in_body = true,
            // PostgreSQL's `CREATE FUNCTION … BEGIN ATOMIC … END`.
            Word::Atomic if engine == Engine::Postgres && self.previous == Some(Word::Begin) => self.in_body = true,
            _ => {}
        }
        self.previous = Some(word);
    }

    fn is_create_trigger(&self) -> bool {
        matches!(self.head.as_slice(), [Word::Create, Word::Trigger, ..] | [Word::Create, Word::Temp, Word::Trigger])
    }
}

/// Ends the current segment at `range.end`: a statement if anything but
/// comments is in it.
fn flush(sql: &str, range: Range<usize>, seg: &mut Segment, out: &mut Vec<Statement>) {
    let has_content = seg.has_content;
    *seg = Segment::default();
    if !has_content {
        return;
    }
    let raw = &sql[range.clone()];
    let text = raw.trim();
    if text.is_empty() {
        return;
    }
    let start = range.start + (raw.len() - raw.trim_start().len());
    out.push(Statement { text: text.to_owned(), range: start..start + text.len() });
}

/// Letters, digits, `_`, any non-ASCII byte; for PostgreSQL also `$`.
fn is_word_byte(c: u8, engine: Engine) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 || (c == b'$' && engine == Engine::Postgres)
}

/// The end of the quoted text opening at `i` (`q` doubled stands for itself;
/// with `backslash`, `\x` is one escaped character). Unterminated: the end.
fn skip_quoted(b: &[u8], i: usize, q: u8, backslash: bool) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        if backslash && b[j] == b'\\' {
            j += 2;
        } else if b[j] == q {
            if b.get(j + 1) == Some(&q) {
                j += 2;
            } else {
                return j + 1;
            }
        } else {
            j += 1;
        }
    }
    b.len()
}

/// `[name]` (SQL Server doubles `]`; SQLite has no escape).
fn skip_bracketed(b: &[u8], i: usize, doubled: bool) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        if b[j] == b']' {
            if doubled && b.get(j + 1) == Some(&b']') {
                j += 2;
            } else {
                return j + 1;
            }
        } else {
            j += 1;
        }
    }
    b.len()
}

/// The end of the block comment opening at `i`; `nested` ones count `/*` inside.
fn skip_block_comment(b: &[u8], i: usize, nested: bool) -> usize {
    let mut depth = 1;
    let mut j = i + 2;
    while j < b.len() {
        if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return j;
            }
        } else if nested && b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
            depth += 1;
            j += 2;
        } else {
            j += 1;
        }
    }
    b.len()
}

/// The end of the `$tag$ … $tag$` opening at `i`, `None` if `$` is not a
/// dollar quote (a parameter `$1`, say).
fn skip_dollar_quoted(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'_' || b[j] >= 0x80) {
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80) {
            j += 1;
        }
    }
    if b.get(j) != Some(&b'$') {
        return None;
    }
    let open = j + 1;
    let tag = &b[i..open];
    Some(match b[open..].windows(tag.len()).position(|w| w == tag) {
        Some(p) => open + p + tag.len(),
        None => b.len(),
    })
}

/// If the line starting at `i` is only `GO` (any case, blanks around, `/* */`
/// and `--` comments after), where it ends (before its newline).
fn go_line(b: &[u8], i: usize) -> Option<usize> {
    let blank = |c: u8| c == b' ' || c == b'\t';
    let mut j = i;
    while j < b.len() && blank(b[j]) {
        j += 1;
    }
    if !b.get(j..j + 2)?.eq_ignore_ascii_case(b"go") {
        return None;
    }
    j += 2;
    loop {
        while j < b.len() && blank(b[j]) {
            j += 1;
        }
        if b.get(j) == Some(&b'/') && b.get(j + 1) == Some(&b'*') {
            j = skip_block_comment(b, j, true);
        } else {
            break;
        }
    }
    match b.get(j) {
        None | Some(b'\n') | Some(b'\r') => Some(j),
        Some(b'-') if b.get(j + 1) == Some(&b'-') => {
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sql: &str, engine: Engine) -> Vec<String> {
        split(sql, engine).into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn semicolons_split_and_ranges_point_back_into_the_source() {
        let sql = "  SELECT 1;\n\nSELECT 2 ;  SELECT 3";
        let all = split(sql, Engine::Postgres);
        assert_eq!(all.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["SELECT 1", "SELECT 2", "SELECT 3"]);
        for s in &all {
            assert_eq!(&sql[s.range.clone()], s.text);
        }
        assert!(split("", Engine::Postgres).is_empty());
        assert!(split(" \n ; ;\n", Engine::Sqlite).is_empty());
    }

    #[test]
    fn comments_and_blanks_alone_are_not_statements() {
        assert!(split("-- nothing\n/* here */ ;", Engine::Postgres).is_empty());
        assert_eq!(texts("SELECT 1; -- done\n/* end */", Engine::Sqlite), ["SELECT 1"]);
        // A comment ahead of a statement stays with it.
        assert_eq!(texts("-- one\nSELECT 1;", Engine::Postgres), ["-- one\nSELECT 1"]);
    }

    #[test]
    fn quotes_comments_and_identifiers_hide_semicolons() {
        for engine in [Engine::Postgres, Engine::Sqlite] {
            assert_eq!(texts("SELECT 'a;b', 'it''s;'; SELECT 2", engine), ["SELECT 'a;b', 'it''s;'", "SELECT 2"]);
            assert_eq!(texts("SELECT \"a;b\"\"\" FROM t; SELECT 2", engine), ["SELECT \"a;b\"\"\" FROM t", "SELECT 2"]);
            assert_eq!(texts("SELECT 1 -- a;b\n; SELECT 2", engine), ["SELECT 1 -- a;b", "SELECT 2"]);
            assert_eq!(texts("SELECT /* a;b */ 1; SELECT 2", engine), ["SELECT /* a;b */ 1", "SELECT 2"]);
        }
    }

    #[test]
    fn postgres_block_comments_nest_and_sqlite_ones_do_not() {
        assert_eq!(
            texts("SELECT /* a /* b; */ c; */ 1; SELECT 2", Engine::Postgres),
            ["SELECT /* a /* b; */ c; */ 1", "SELECT 2"]
        );
        assert_eq!(texts("SELECT /* a /* b; */ 1; SELECT 2", Engine::Sqlite), ["SELECT /* a /* b; */ 1", "SELECT 2"]);
    }

    #[test]
    fn postgres_dollar_quotes_and_escape_strings() {
        let body = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql; SELECT 2";
        assert_eq!(
            texts(body, Engine::Postgres),
            ["CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql", "SELECT 2"]
        );
        assert_eq!(
            texts("DO $fx$ BEGIN PERFORM $$;$$; END $fx$; SELECT 2", Engine::Postgres),
            ["DO $fx$ BEGIN PERFORM $$;$$; END $fx$", "SELECT 2"]
        );
        // `$1` is a parameter, `a$b` an identifier: neither opens a quote.
        assert_eq!(
            texts("SELECT $1; SELECT a$b$ FROM t; SELECT 3", Engine::Postgres),
            ["SELECT $1", "SELECT a$b$ FROM t", "SELECT 3"]
        );
        // E'' takes a backslash escape, plain '' does not.
        assert_eq!(texts(r"SELECT E'it\'s;'; SELECT 2", Engine::Postgres), [r"SELECT E'it\'s;'", "SELECT 2"]);
        assert_eq!(texts(r"SELECT 'a\'; SELECT 2", Engine::Postgres), [r"SELECT 'a\'", "SELECT 2"]);
        assert_eq!(texts("SELECT 1 AS be; SELECT 'x;'", Engine::Postgres), ["SELECT 1 AS be", "SELECT 'x;'"]);
    }

    #[test]
    fn postgres_begin_atomic_bodies_stay_whole() {
        let f =
            "CREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT CASE WHEN true THEN 1 END; SELECT 2; END";
        assert_eq!(texts(&format!("{f}; SELECT 3"), Engine::Postgres), [f, "SELECT 3"]);
        assert_eq!(texts("BEGIN; SELECT 1; END", Engine::Postgres), ["BEGIN", "SELECT 1", "END"]);
    }

    #[test]
    fn sqlite_brackets_backticks_and_trigger_bodies() {
        assert_eq!(texts("SELECT [a;b] FROM t; SELECT `c;d`", Engine::Sqlite), ["SELECT [a;b] FROM t", "SELECT `c;d`"]);
        let trigger = "CREATE TRIGGER t AFTER INSERT ON a WHEN NEW.x > 0 BEGIN INSERT INTO b VALUES (NEW.x); \
                       UPDATE c SET n = CASE WHEN NEW.x > 1 THEN 1 ELSE 0 END; END";
        assert_eq!(texts(&format!("{trigger}; SELECT 1"), Engine::Sqlite), [trigger, "SELECT 1"]);
        let temp = "CREATE TEMP TRIGGER t AFTER DELETE ON a BEGIN DELETE FROM b; END";
        assert_eq!(texts(&format!("{temp};\nSELECT 1"), Engine::Sqlite), [temp, "SELECT 1"]);
        // A transaction's BEGIN/END are plain statements.
        assert_eq!(
            texts("BEGIN; INSERT INTO a VALUES (1); END;", Engine::Sqlite),
            ["BEGIN", "INSERT INTO a VALUES (1)", "END"]
        );
        // A column or table named end_date is not END.
        let t2 = "CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET end_date = 1; END";
        assert_eq!(texts(&format!("{t2}; SELECT 1"), Engine::Sqlite), [t2, "SELECT 1"]);
    }

    #[test]
    fn go_lines_may_carry_comments() {
        let ms = |sql| texts(sql, Engine::Mssql);
        assert_eq!(ms("SELECT 1\nGO /* next batch */\nSELECT 2"), ["SELECT 1", "SELECT 2"]);
        assert_eq!(ms("SELECT 1\ngo\t/* a */ /* b */ -- c\r\nSELECT 2"), ["SELECT 1", "SELECT 2"]);
        assert_eq!(ms("SELECT 1\nGO /* spans\nlines */\nSELECT 2"), ["SELECT 1", "SELECT 2"]);
        // Something after the comment: not a GO line.
        assert_eq!(ms("SELECT 1\nGO /* x */ SELECT 2"), ["SELECT 1\nGO /* x */ SELECT 2"]);
    }

    #[test]
    fn postgres_backslashes_escape_in_ordinary_strings_when_the_session_says_so() {
        let sql = "SELECT 'it\\'; DELETE FROM t; SELECT 2'; SELECT 3";
        assert_eq!(
            split_with(sql, Engine::Postgres, true).into_iter().map(|s| s.text).collect::<Vec<_>>(),
            ["SELECT 'it\\'; DELETE FROM t; SELECT 2'", "SELECT 3"]
        );
        // standard_conforming_strings on (the default): `\` is just a character.
        assert_eq!(texts(sql, Engine::Postgres), ["SELECT 'it\\'", "DELETE FROM t", "SELECT 2'; SELECT 3"]);
        // Identifiers and dollar quotes never take backslash escapes.
        assert_eq!(
            split_with("SELECT \"a\\\"; SELECT $$\\$$; SELECT 3", Engine::Postgres, true)
                .into_iter()
                .map(|s| s.text)
                .collect::<Vec<_>>(),
            ["SELECT \"a\\\"", "SELECT $$\\$$", "SELECT 3"]
        );
        // After these the session is asked again.
        for statement in ["set standard_conforming_strings = off", "RESET ALL", "DISCARD ALL", "rollback", "COMMIT"] {
            assert!(may_change_string_escapes(statement), "{statement}");
        }
        assert!(!may_change_string_escapes("SET search_path = app"));
        assert!(!may_change_string_escapes("SELECT 'reset'"));
    }

    #[test]
    fn mssql_splits_on_go_lines_only() {
        let sql = "SELECT 1;\nSELECT 2\nGO\nSELECT 3\n  go  -- next\nSELECT [GO]\nGO";
        assert_eq!(texts(sql, Engine::Mssql), ["SELECT 1;\nSELECT 2", "SELECT 3", "SELECT [GO]"]);
        // GO inside text, a comment, a longer word or a statement is not a separator.
        assert_eq!(texts("SELECT 'a\nGO\nb'", Engine::Mssql), ["SELECT 'a\nGO\nb'"]);
        assert_eq!(texts("/*\nGO\n*/ SELECT 1", Engine::Mssql), ["/*\nGO\n*/ SELECT 1"]);
        assert_eq!(
            texts("SELECT 1\nGOTO x\nGO 5\nSELECT 2 GO", Engine::Mssql),
            ["SELECT 1\nGOTO x\nGO 5\nSELECT 2 GO"]
        );
        assert_eq!(texts("SELECT 1\r\nGO\r\nSELECT 2", Engine::Mssql), ["SELECT 1", "SELECT 2"]);
        assert!(split("GO\n\nGO", Engine::Mssql).is_empty());
    }

    #[test]
    fn mssql_brackets_and_n_strings() {
        assert_eq!(
            texts("SELECT [a]]b], N'it''s\nGO\n'\nGO\nSELECT 2", Engine::Mssql),
            ["SELECT [a]]b], N'it''s\nGO\n'", "SELECT 2"]
        );
        // `;` does not split a batch.
        assert_eq!(
            texts("DECLARE @a int; SET @a = 1; SELECT @a", Engine::Mssql),
            ["DECLARE @a int; SET @a = 1; SELECT @a"]
        );
        // Block comments nest.
        assert_eq!(texts("SELECT /* a /* b\nGO\n*/ c */ 1", Engine::Mssql), ["SELECT /* a /* b\nGO\n*/ c */ 1"]);
    }

    #[test]
    fn unterminated_text_runs_to_the_end() {
        assert_eq!(texts("SELECT 'a; SELECT 2", Engine::Postgres), ["SELECT 'a; SELECT 2"]);
        assert_eq!(texts("SELECT /* a; SELECT 2", Engine::Sqlite), ["SELECT /* a; SELECT 2"]);
        assert_eq!(texts("SELECT $$a; SELECT 2", Engine::Postgres), ["SELECT $$a; SELECT 2"]);
    }

    #[test]
    fn multibyte_text_keeps_its_boundaries() {
        let sql = "SELECT '資料;庫'; SELECT 名稱 FROM 表";
        let all = split(sql, Engine::Postgres);
        assert_eq!(
            all.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            ["SELECT '資料;庫'", "SELECT 名稱 FROM 表"]
        );
        for s in &all {
            assert_eq!(&sql[s.range.clone()], s.text);
        }
    }

    #[test]
    fn the_cursor_picks_its_statement() {
        let sql = "SELECT 1;\n\n  SELECT 2;\n-- tail";
        let at = |offset| statement_at(sql, offset, Engine::Postgres).map(|s| s.text);
        assert_eq!(at(0).as_deref(), Some("SELECT 1"));
        assert_eq!(at(8).as_deref(), Some("SELECT 1"));
        // On the `;` and in the blank lines after it: still the one before.
        assert_eq!(at(9).as_deref(), Some("SELECT 1"));
        assert_eq!(at(11).as_deref(), Some("SELECT 1"));
        assert_eq!(at(14).as_deref(), Some("SELECT 2"));
        assert_eq!(at(sql.len()).as_deref(), Some("SELECT 2"));
        // Blank space ahead of the first statement: the first.
        assert_eq!(statement_at("\n\nSELECT 1", 0, Engine::Sqlite).map(|s| s.text).as_deref(), Some("SELECT 1"));
        assert_eq!(statement_at("-- only", 3, Engine::Sqlite), None);
        let go = "SELECT 1\nGO\nSELECT 2";
        assert_eq!(statement_at(go, 10, Engine::Mssql).map(|s| s.text).as_deref(), Some("SELECT 1"));
        assert_eq!(statement_at(go, 14, Engine::Mssql).map(|s| s.text).as_deref(), Some("SELECT 2"));
    }

    #[test]
    fn the_cursor_runs_nothing_whose_end_the_string_mode_decides() {
        let sql = "SELECT 'it\\'; DELETE FROM important; SELECT 2'; SELECT 3";
        let delete = sql.find("DELETE").unwrap();
        assert_eq!(runnable_statement_at(sql, delete, Engine::Postgres), None);
        assert_eq!(runnable_statement_at(sql, sql.len(), Engine::Postgres), None);
        // Both readings agree: it runs.
        let sql = "SELECT 'a\\b'; SELECT 2";
        assert_eq!(runnable_statement_at(sql, 0, Engine::Postgres).as_deref(), Some("SELECT 'a\\b'"));
        assert_eq!(runnable_statement_at(sql, sql.len(), Engine::Postgres).as_deref(), Some("SELECT 2"));
        assert_eq!(runnable_statement_at("-- only", 3, Engine::Postgres).as_deref(), Some(""));
        // Other engines have one reading.
        let sql = "SELECT 'it\\'; SELECT 2";
        assert_eq!(runnable_statement_at(sql, 0, Engine::Sqlite).as_deref(), Some("SELECT 'it\\'"));
    }

    #[test]
    fn row_changing_statements_are_told_from_the_rest() {
        for s in [
            "INSERT INTO t VALUES (1)",
            "  update t set a = 1",
            "-- c\nDELETE FROM t",
            "/* c */ WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
        ] {
            assert!(changes_rows(s), "{s}");
        }
        for s in ["CREATE TABLE t (a int)", "SET search_path = x", "BEGIN", "DROP TABLE t", "", "-- only"] {
            assert!(!changes_rows(s), "{s}");
        }
    }

    #[test]
    fn a_mismatched_key_is_explained() {
        let key = vec![("id".to_owned(), Cell::Int(7)), ("kind".to_owned(), Cell::text("a"))];
        let none = key_mismatch(&key, 0);
        assert!(none.contains("id = 7, kind = a") && none.contains('0'), "{none}");
        assert!(key_mismatch(&key, 2).contains('2'));
    }

    #[test]
    fn objects_sort_by_schema_kind_then_name() {
        let o = |schema: &str, name: &str, kind| DbObject {
            schema: schema.into(),
            name: name.into(),
            kind,
            rows: None,
            detail: String::new(),
        };
        let mut list = vec![
            o("b", "a", ObjectKind::Table),
            o("a", "z", ObjectKind::View),
            o("a", "y", ObjectKind::Function),
            o("a", "b", ObjectKind::Table),
            o("a", "a", ObjectKind::Table),
        ];
        sort_objects(&mut list);
        let names: Vec<_> = list.iter().map(|o| format!("{}.{}", o.schema, o.name)).collect();
        assert_eq!(names, ["a.a", "a.b", "a.z", "a.y", "b.a"]);
    }
}
