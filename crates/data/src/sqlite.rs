//! A read-only reader of SQLite database files, in pure Rust: table b-trees, records and overflow
//! pages, and the column lists of `CREATE TABLE` statements. It is what the browser opens
//! GeoPackages with (SQLite itself is C); the desktop uses SQLite.
//!
//! No SQL is run: tables are scanned in rowid order. The file is hostile: every page number,
//! offset and length is checked against the bytes there, a page is visited at most once per scan
//! (so a cycle of pages ends), and a malformed page is an error, never a panic.

use std::ops::ControlFlow;

/// A value in a row, as stored: SQLite stores a whole-number REAL as an integer, so readers
/// convert by the column's declared type.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// A column of a table, as its `CREATE TABLE` declares it.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    /// The declared type (`INTEGER`, `TEXT`, `POINT`…), possibly empty.
    pub decl: String,
    /// A virtual generated column: not stored in the rows.
    pub generated: bool,
}

/// A table of the database.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    /// The root page of its b-tree.
    pub root: u32,
    pub columns: Vec<Column>,
    /// The `INTEGER PRIMARY KEY` column, which is the rowid (stored as NULL in the rows).
    pub rowid_column: Option<usize>,
}

/// An error reading the file.
fn bad(what: &str) -> String {
    format!("not a readable SQLite database: {what}")
}

/// An SQLite database held in memory.
pub struct Db<B: AsRef<[u8]>> {
    bytes: B,
    page_size: usize,
    /// Bytes of each page the b-tree uses (the page size less the reserved bytes at its end).
    usable: usize,
    pages: u32,
}

impl<B: AsRef<[u8]>> Db<B> {
    /// Opens the database in `bytes`, checking its header.
    pub fn open(bytes: B) -> Result<Self, String> {
        let b = bytes.as_ref();
        if b.get(..16) != Some(b"SQLite format 3\0") {
            return Err("not an SQLite database".into());
        }
        let page_size = match be16(b, 16).ok_or_else(|| bad("header"))? {
            1 => 65_536,
            n if n >= 512 && n.is_power_of_two() => n,
            _ => return Err(bad("page size")),
        };
        let reserved = usize::from(*b.get(20).ok_or_else(|| bad("header"))?);
        let usable = page_size.checked_sub(reserved).filter(|&u| u >= 480).ok_or_else(|| bad("reserved space"))?;
        match be32(b, 56).ok_or_else(|| bad("header"))? {
            0 | 1 => {}
            _ => return Err("SQLite databases in UTF-16 aren't read".into()),
        }
        let pages = u32::try_from(b.len() / page_size).map_err(|_| bad("size"))?;
        if pages == 0 {
            return Err(bad("no pages"));
        }
        Ok(Db { bytes, page_size, usable, pages })
    }

    /// Page `n` (numbered from 1).
    fn page(&self, n: u32) -> Result<&[u8], String> {
        if n == 0 || n > self.pages {
            return Err(bad("a page number past the end of the file"));
        }
        let start = (n as usize - 1).checked_mul(self.page_size).ok_or_else(|| bad("page offset"))?;
        self.bytes.as_ref().get(start..start + self.page_size).ok_or_else(|| bad("a truncated page"))
    }

    /// Calls `f` with the rowid and values of every row of the table b-tree rooted at `root`, in
    /// rowid order, until it returns `Break`.
    pub fn scan(&self, root: u32, mut f: impl FnMut(i64, Vec<Val>) -> ControlFlow<()>) -> Result<(), String> {
        let mut seen = vec![false; self.pages as usize + 1];
        let mut stack = vec![root];
        // Depth-first, children pushed right to left so rows come out in rowid order.
        while let Some(n) = stack.pop() {
            let slot = seen.get_mut(n as usize).ok_or_else(|| bad("a page number past the end of the file"))?;
            if std::mem::replace(slot, true) {
                return Err(bad("a page reached twice"));
            }
            let page = self.page(n)?;
            let hdr = if n == 1 { 100 } else { 0 };
            let kind = *page.get(hdr).ok_or_else(|| bad("page header"))?;
            let cells = be16(page, hdr + 3).ok_or_else(|| bad("page header"))?;
            match kind {
                // An interior table page: child pointers, then the right-most child.
                0x05 => {
                    let mut children = Vec::with_capacity(cells + 1);
                    for i in 0..cells {
                        let at = be16(page, hdr + 12 + 2 * i).ok_or_else(|| bad("cell pointer"))?;
                        children.push(be32(page, at).ok_or_else(|| bad("interior cell"))?);
                    }
                    children.push(be32(page, hdr + 8).ok_or_else(|| bad("page header"))?);
                    stack.extend(children.into_iter().rev());
                }
                // A leaf table page: (payload size, rowid, payload) cells.
                0x0d => {
                    for i in 0..cells {
                        let at = be16(page, hdr + 8 + 2 * i).ok_or_else(|| bad("cell pointer"))?;
                        let (size, a) = varint(page, at).ok_or_else(|| bad("leaf cell"))?;
                        let (rowid, a) = varint(page, a).ok_or_else(|| bad("leaf cell"))?;
                        let payload = self.payload(page, a, size)?;
                        if f(rowid as i64, record(&payload)?).is_break() {
                            return Ok(());
                        }
                    }
                }
                _ => return Err(bad("a table page of the wrong kind")),
            }
        }
        Ok(())
    }

    /// The number of rows in the table b-tree rooted at `root`, from the leaf pages' cell counts
    /// (no row is decoded).
    pub fn count(&self, root: u32) -> Result<u64, String> {
        let mut seen = vec![false; self.pages as usize + 1];
        let mut stack = vec![root];
        let mut n_rows = 0u64;
        while let Some(n) = stack.pop() {
            let slot = seen.get_mut(n as usize).ok_or_else(|| bad("a page number past the end of the file"))?;
            if std::mem::replace(slot, true) {
                return Err(bad("a page reached twice"));
            }
            let page = self.page(n)?;
            let hdr = if n == 1 { 100 } else { 0 };
            let cells = be16(page, hdr + 3).ok_or_else(|| bad("page header"))?;
            match page.get(hdr) {
                Some(0x05) => {
                    for i in 0..cells {
                        let at = be16(page, hdr + 12 + 2 * i).ok_or_else(|| bad("cell pointer"))?;
                        stack.push(be32(page, at).ok_or_else(|| bad("interior cell"))?);
                    }
                    stack.push(be32(page, hdr + 8).ok_or_else(|| bad("page header"))?);
                }
                Some(0x0d) => n_rows += cells as u64,
                _ => return Err(bad("a table page of the wrong kind")),
            }
        }
        Ok(n_rows)
    }

    /// A cell's payload of `size` bytes starting at `at` on `page`, following its overflow pages.
    fn payload(&self, page: &[u8], at: usize, size: u64) -> Result<Vec<u8>, String> {
        let total = self.bytes.as_ref().len() as u64;
        if size > total {
            return Err(bad("a payload larger than the file"));
        }
        let size = size as usize;
        let u = self.usable;
        // How much of the payload is on the leaf page itself (the SQLite file format's rule).
        let max_local = u - 35;
        let local = if size <= max_local {
            size
        } else {
            let min_local = (u - 12) * 32 / 255 - 23;
            let k = min_local + (size - min_local) % (u - 4);
            if k <= max_local { k } else { min_local }
        };
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(page.get(at..at + local).ok_or_else(|| bad("a cell past its page"))?);
        if local < size {
            let mut next = be32(page, at + local).ok_or_else(|| bad("overflow pointer"))?;
            let mut hops = 0u32;
            while out.len() < size {
                hops += 1;
                if next == 0 || hops > self.pages {
                    return Err(bad("an overflow chain that ends early"));
                }
                let p = self.page(next)?;
                let take = (size - out.len()).min(u - 4);
                out.extend_from_slice(p.get(4..4 + take).ok_or_else(|| bad("overflow page"))?);
                next = be32(p, 0).ok_or_else(|| bad("overflow page"))?;
            }
        }
        Ok(out)
    }

    /// The tables, from the schema table (`sqlite_schema`, page 1).
    pub fn tables(&self) -> Result<Vec<Table>, String> {
        let mut out = Vec::new();
        let mut err = None;
        self.scan(1, |_, row| {
            if let [Val::Text(kind), Val::Text(name), _, Val::Int(root), Val::Text(sql), ..] = row.as_slice()
                && kind == "table"
            {
                // WITHOUT ROWID or unparsable tables are passed over; another may be wanted.
                if let Ok((columns, rowid_column)) = parse_create_table(sql) {
                    match u32::try_from(*root) {
                        Ok(root) => out.push(Table { name: name.clone(), root, columns, rowid_column }),
                        Err(_) => err = Some(bad("a table's root page")),
                    }
                }
            }
            ControlFlow::Continue(())
        })?;
        match err {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// A table by name, without regard to case (as SQLite looks names up).
    pub fn table(&self, name: &str) -> Result<Table, String> {
        self.tables()?.into_iter().find(|t| t.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no table '{name}'"))
    }

    /// Calls `f` with each row of `table` as one value per column (the rowid in its
    /// `INTEGER PRIMARY KEY` column; columns added after the row was written are NULL), until it
    /// returns `Break`.
    pub fn rows(&self, table: &Table, mut f: impl FnMut(i64, &[Val]) -> ControlFlow<()>) -> Result<(), String> {
        let mut row = Vec::with_capacity(table.columns.len());
        self.scan(table.root, |rowid, stored| {
            row.clear();
            let mut stored = stored.into_iter();
            for (i, c) in table.columns.iter().enumerate() {
                let v = if c.generated { Val::Null } else { stored.next().unwrap_or(Val::Null) };
                row.push(if Some(i) == table.rowid_column { Val::Int(rowid) } else { v });
            }
            f(rowid, &row)
        })
    }
}

fn be16(b: &[u8], at: usize) -> Option<usize> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(usize::from(u16::from_be_bytes([*s.first()?, *s.get(1)?])))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    let s: [u8; 4] = b.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(s))
}

/// An SQLite varint at `at`: the value and the offset after it.
fn varint(b: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for i in 0..9 {
        let byte = *b.get(at.checked_add(i)?)?;
        if i == 8 {
            return Some(((v << 8) | u64::from(byte), at + 9));
        }
        v = (v << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((v, at + i + 1));
        }
    }
    None
}

/// The values of a record: a header of serial types, then the values.
fn record(p: &[u8]) -> Result<Vec<Val>, String> {
    let (header_len, mut h) = varint(p, 0).ok_or_else(|| bad("record header"))?;
    let header_len = usize::try_from(header_len).ok().filter(|&n| n <= p.len()).ok_or_else(|| bad("record header"))?;
    let mut body = header_len;
    let mut out = Vec::new();
    while h < header_len {
        let (t, next) = varint(p, h).ok_or_else(|| bad("record header"))?;
        h = next;
        let int = |n: usize| -> Result<i64, String> {
            let s = p.get(body..body + n).ok_or_else(|| bad("a record past its payload"))?;
            // Big-endian two's complement of 1 to 8 bytes, sign-extended.
            let first = i64::from(*s.first().ok_or_else(|| bad("record"))? as i8);
            Ok(s.iter().skip(1).fold(first, |v, &b| (v << 8) | i64::from(b)))
        };
        let (v, len) = match t {
            0 => (Val::Null, 0),
            1..=4 => (Val::Int(int(t as usize)?), t as usize),
            5 => (Val::Int(int(6)?), 6),
            6 => (Val::Int(int(8)?), 8),
            7 => {
                let s: [u8; 8] = p.get(body..body + 8).and_then(|s| s.try_into().ok()).ok_or_else(|| bad("a record past its payload"))?;
                (Val::Real(f64::from_be_bytes(s)), 8)
            }
            8 => (Val::Int(0), 0),
            9 => (Val::Int(1), 0),
            10 | 11 => return Err(bad("a reserved serial type")),
            t => {
                let n = usize::try_from((t - 12) / 2).map_err(|_| bad("record"))?;
                let s = p.get(body..body.checked_add(n).ok_or_else(|| bad("record"))?).ok_or_else(|| bad("a record past its payload"))?;
                (if t % 2 == 0 { Val::Blob(s.to_vec()) } else { Val::Text(String::from_utf8_lossy(s).into_owned()) }, n)
            }
        };
        body += len;
        out.push(v);
    }
    Ok(out)
}

/// The columns of a `CREATE TABLE` statement and the index of its `INTEGER PRIMARY KEY` column.
/// `WITHOUT ROWID` tables (whose rows are stored differently) are an error.
pub fn parse_create_table(sql: &str) -> Result<(Vec<Column>, Option<usize>), String> {
    let open = sql.find('(').ok_or("no column list")?;
    let close = sql.rfind(')').filter(|&c| c > open).ok_or("no column list")?;
    let tail = sql.get(close + 1..).unwrap_or("").to_ascii_uppercase();
    if tail.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "ROWID") {
        return Err("a WITHOUT ROWID table".into());
    }
    let body = sql.get(open + 1..close).ok_or("no column list")?;
    let mut columns = Vec::new();
    let mut rowid_column = None;
    let mut table_pk: Option<Vec<String>> = None;
    for def in split_top(body) {
        let toks = tokens(&def);
        let Some(first) = toks.first() else { continue };
        let up = |s: &str| s.to_ascii_uppercase();
        if !first.quoted && matches!(up(&first.text).as_str(), "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN") {
            // A table constraint; a PRIMARY KEY (one column) may name the rowid's alias.
            if let Some(p) = toks.iter().position(|t| !t.quoted && up(&t.text) == "PRIMARY") {
                let cols: Vec<String> = toks
                    .iter()
                    .skip(p + 2)
                    .filter(|t| t.quoted || !matches!(t.text.as_str(), "(" | ")" | ","))
                    .map(|t| t.text.clone())
                    .take_while(|t| !matches!(up(t).as_str(), "ASC" | "DESC" | "ON" | "CONFLICT"))
                    .collect();
                table_pk = Some(cols);
            }
            continue;
        }
        const STOP: &[&str] =
            &["CONSTRAINT", "PRIMARY", "NOT", "NULL", "UNIQUE", "CHECK", "DEFAULT", "COLLATE", "REFERENCES", "GENERATED", "AS"];
        let ty: Vec<&str> =
            toks.iter().skip(1).take_while(|t| t.quoted || !STOP.contains(&up(&t.text).as_str())).map(|t| t.text.as_str()).collect();
        let decl = ty.join(" ").replace(" (", "(").replace("( ", "(").replace(" )", ")").replace(" ,", ",");
        let words: Vec<String> = toks.iter().filter(|t| !t.quoted).map(|t| up(&t.text)).collect();
        let has = |w: &str| words.iter().any(|x| x == w);
        // A generated column is stored only when declared STORED.
        let generated = (has("GENERATED") || has("AS")) && !has("STORED");
        if decl.eq_ignore_ascii_case("INTEGER") && has("PRIMARY") && !has("DESC") {
            rowid_column = Some(columns.len());
        }
        columns.push(Column { name: first.text.clone(), decl, generated });
    }
    if rowid_column.is_none()
        && let Some([pk]) = table_pk.as_deref()
    {
        rowid_column = columns.iter().position(|c| c.name.eq_ignore_ascii_case(pk) && c.decl.eq_ignore_ascii_case("INTEGER"));
    }
    if columns.is_empty() {
        return Err("no columns".into());
    }
    Ok((columns, rowid_column))
}

/// Splits on commas outside parentheses and quotes.
fn split_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q || (q == '[' && c == ']') {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' | '`' | '[' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    cur.push(c);
                }
                ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            },
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

struct Token {
    text: String,
    quoted: bool,
}

/// Words, quoted names (unquoted, with doubled quotes undone) and punctuation of a definition;
/// `--` and `/* */` comments are dropped.
fn tokens(s: &str) -> Vec<Token> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = cs.get(i) {
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && cs.get(i + 1) == Some(&'-') {
            while cs.get(i).is_some_and(|&c| c != '\n') {
                i += 1;
            }
        } else if c == '/' && cs.get(i + 1) == Some(&'*') {
            i += 2;
            while i < cs.len() && !(cs.get(i) == Some(&'*') && cs.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if matches!(c, '"' | '\'' | '`' | '[') {
            let close = if c == '[' { ']' } else { c };
            let mut text = String::new();
            i += 1;
            while let Some(&d) = cs.get(i) {
                if d == close {
                    if close != ']' && cs.get(i + 1) == Some(&close) {
                        text.push(d);
                        i += 2;
                        continue;
                    }
                    break;
                }
                text.push(d);
                i += 1;
            }
            i += 1;
            out.push(Token { text, quoted: true });
        } else if c.is_alphanumeric() || c == '_' || c == '$' {
            let start = i;
            while cs.get(i).is_some_and(|&d| d.is_alphanumeric() || d == '_' || d == '$') {
                i += 1;
            }
            out.push(Token { text: cs.get(start..i).map(|s| s.iter().collect()).unwrap_or_default(), quoted: false });
        } else {
            out.push(Token { text: c.to_string(), quoted: false });
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_table_columns() {
        let (cols, pk) = parse_create_table(
            r#"CREATE TABLE "my ""t""" ( fid INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, "geom" MULTIPOLYGON, [name] TEXT(80) DEFAULT 'a,b', `n` DOUBLE PRECISION, -- note
               x NUMERIC(10, 2), g TEXT GENERATED ALWAYS AS (name || 'x') VIRTUAL, CONSTRAINT c UNIQUE (name))"#,
        )
        .unwrap();
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["fid", "geom", "name", "n", "x", "g"]);
        assert_eq!(cols[1].decl, "MULTIPOLYGON");
        assert_eq!(cols[2].decl, "TEXT(80)");
        assert_eq!(cols[3].decl, "DOUBLE PRECISION");
        assert_eq!(cols[4].decl, "NUMERIC(10, 2)");
        assert!(cols[5].generated && !cols[2].generated);
        assert_eq!(pk, Some(0));
        let (_, pk) = parse_create_table("CREATE TABLE t (a TEXT, id INTEGER, PRIMARY KEY (id))").unwrap();
        assert_eq!(pk, Some(1));
        let (_, pk) = parse_create_table("CREATE TABLE t (id INT PRIMARY KEY, b)").unwrap();
        assert_eq!(pk, None, "only INTEGER is the rowid");
        assert!(parse_create_table("CREATE TABLE t (a INTEGER PRIMARY KEY) WITHOUT ROWID").is_err());
        assert!(parse_create_table("CREATE TABLE t").is_err());
    }

    #[test]
    fn varints_and_records() {
        assert_eq!(varint(&[0x7f], 0), Some((127, 1)));
        assert_eq!(varint(&[0x81, 0x00], 0), Some((128, 2)));
        assert_eq!(varint(&[0xff; 9], 0), Some((u64::MAX, 9)));
        assert_eq!(varint(&[0x81], 0), None);
        // Header of 5 bytes: NULL, a 1-byte int, a 2-byte int, a float, text "hi".
        let mut rec = vec![6, 0, 1, 2, 7, 17, 0xfe, 0x01, 0x00];
        rec.extend_from_slice(&1.5f64.to_be_bytes());
        rec.extend_from_slice(b"hi");
        assert_eq!(record(&rec).unwrap(), vec![Val::Null, Val::Int(-2), Val::Int(256), Val::Real(1.5), Val::Text("hi".into())]);
        assert!(record(&rec[..rec.len() - 1]).is_err());
        assert!(record(&[2, 10]).is_err());
    }
    /// Databases written by SQLite itself: a table spanning interior pages, rows with overflow
    /// pages, a column added after rows were written, small and large pages; every truncation is
    /// an error, not a panic.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn reads_what_sqlite_writes() {
        for page_size in [512, 4096, 65_536] {
            let path = std::env::temp_dir().join(format!("rustgis-sqlite-{}-{page_size}.db", std::process::id()));
            let _ = std::fs::remove_file(&path);
            {
                let c = rusqlite::Connection::open(&path).unwrap();
                c.execute_batch(&format!(
                    "PRAGMA page_size = {page_size}; PRAGMA journal_mode = DELETE;
                     CREATE TABLE \"t\" (id INTEGER PRIMARY KEY, name TEXT, v REAL, b BLOB, n INTEGER);
                     CREATE TABLE other (x);"
                ))
                .unwrap();
                let tx = c.unchecked_transaction().unwrap();
                for i in 0..3000i64 {
                    // Every 97th row carries a blob larger than a page.
                    let blob: Option<Vec<u8>> = (i % 97 == 0).then(|| (0..page_size * 3 + 7).map(|k| (k % 251) as u8).collect());
                    let n: Option<i64> = (i % 5 != 0).then_some(i * 1_000_000_007);
                    tx.execute(
                        "INSERT INTO t (id, name, v, b, n) VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![i * 3 - 100, format!("row {i} é"), i as f64 / 7.0, blob, n],
                    )
                    .unwrap();
                }
                tx.commit().unwrap();
                c.execute_batch("ALTER TABLE t ADD COLUMN later TEXT; INSERT INTO t (id, name, later) VALUES (999999, 'last', 'x');")
                    .unwrap();
            }
            let bytes = std::fs::read(&path).unwrap();
            let db = Db::open(bytes.as_slice()).unwrap();
            let tables = db.tables().unwrap();
            assert_eq!(tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["t", "other"]);
            let t = db.table("T").unwrap();
            assert_eq!(t.rowid_column, Some(0));
            assert_eq!(t.columns.len(), 6);
            assert_eq!(db.count(t.root).unwrap(), 3001);
            let mut rows = Vec::new();
            db.rows(&t, |_, r| {
                rows.push(r.to_vec());
                ControlFlow::Continue(())
            })
            .unwrap();
            assert_eq!(rows.len(), 3001, "{page_size}");
            for (i, r) in rows.iter().take(3000).enumerate() {
                let i = i as i64;
                assert_eq!(r[0], Val::Int(i * 3 - 100));
                assert_eq!(r[1], Val::Text(format!("row {i} é")));
                // SQLite stores a whole-number REAL as an integer; readers convert by the column's type.
                let v = match r[2] {
                    Val::Real(x) => x,
                    Val::Int(x) => x as f64,
                    ref other => panic!("{other:?}"),
                };
                assert_eq!(v, i as f64 / 7.0);
                let blob = if i % 97 == 0 { Val::Blob((0..page_size * 3 + 7).map(|k| (k % 251) as u8).collect()) } else { Val::Null };
                assert_eq!(r[3], blob, "{page_size} row {i}");
                assert_eq!(r[4], if i % 5 != 0 { Val::Int(i * 1_000_000_007) } else { Val::Null });
                assert_eq!(r[5], Val::Null, "the added column");
            }
            assert_eq!(rows[3000][5], Val::Text("x".into()));
            // Stopping early.
            let mut n = 0;
            db.rows(&t, |_, _| {
                n += 1;
                if n == 10 { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
            })
            .unwrap();
            assert_eq!(n, 10);
            if page_size == 512 {
                for cut in (0..bytes.len()).step_by(211) {
                    if let Ok(db) = Db::open(&bytes[..cut]) {
                        let _ = db.tables();
                        let _ = db.rows(&t, |_, _| ControlFlow::Continue(()));
                    }
                }
                // Flipped bytes in every page.
                let mut junk = bytes.clone();
                for k in (100..junk.len()).step_by(37) {
                    junk[k] ^= 0x5a;
                }
                if let Ok(db) = Db::open(junk.as_slice()) {
                    let _ = db.tables();
                    let _ = db.rows(&t, |_, _| ControlFlow::Continue(()));
                    let _ = db.count(t.root);
                }
            }
            let _ = std::fs::remove_file(&path);
        }
        assert!(Db::open(&b"SQLite format 3\0"[..]).is_err());
        assert!(Db::open(&b"nope"[..]).is_err());
    }
}
