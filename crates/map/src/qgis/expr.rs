//! QGIS expressions as RustGIS SQL clauses: the part of QGIS's expression language a filter
//! mostly is (fields compared with values, `LIKE`, `IS NULL`, `IN`, joined by `AND`, `OR` and
//! `NOT`) carried over as it is, and anything else (functions, arithmetic, `CASE`, variables)
//! refused rather than guessed at, so the caller can note it.

/// The most characters of an expression read.
const MAX_LEN: usize = 10_000;

#[derive(Clone, Debug, PartialEq)]
enum Token {
    /// A field name: written back double-quoted.
    Field(String),
    /// A text value: written back single-quoted.
    Text(String),
    /// A number, a comparison, a keyword, a bracket or a comma: written back as it is.
    Word(String),
}

/// `expr` as a RustGIS SQL clause, or `None` when it uses more of QGIS's language than that has.
/// The clause is checked against a layer's fields where it is used.
pub fn to_sql(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() || expr.len() > MAX_LEN {
        return None;
    }
    let tokens = tokenize(expr)?;
    let mut out = Vec::with_capacity(tokens.len());
    for t in &tokens {
        out.push(match t {
            Token::Field(f) => format!("\"{}\"", f.replace('"', "")),
            Token::Text(s) => format!("'{}'", s.replace('\'', "''")),
            Token::Word(w) => w.clone(),
        });
    }
    Some(out.join(" "))
}

/// The field and values of a filter that only picks values of one field: `"f" = 'v'`, `f = 3`,
/// `"f" IN ('a', 'b')`. `None` for any other.
pub(super) fn equality(expr: &str) -> Option<(String, Vec<String>)> {
    let tokens = tokenize(expr.trim())?;
    let value = |t: &Token| match t {
        Token::Text(s) => Some(s.clone()),
        Token::Word(w) if w.parse::<f64>().is_ok() => Some(w.clone()),
        _ => None,
    };
    match tokens.as_slice() {
        [Token::Field(f), Token::Word(eq), v] if eq == "=" => Some((f.clone(), vec![value(v)?])),
        [Token::Field(f), Token::Word(kw), Token::Word(open), rest @ .., Token::Word(close)]
            if kw == "IN" && open == "(" && close == ")" =>
        {
            let values: Vec<String> = rest.iter().step_by(2).map(value).collect::<Option<_>>()?;
            let commas_ok = rest.iter().skip(1).step_by(2).all(|t| *t == Token::Word(",".into()));
            (commas_ok && !values.is_empty()).then(|| (f.clone(), values))
        }
        _ => None,
    }
}

/// The words RustGIS's SQL has (`ILIKE` is its `LIKE`, which ignores case).
fn keyword(w: &str) -> Option<&'static str> {
    Some(match w.to_ascii_uppercase().as_str() {
        "AND" => "AND",
        "OR" => "OR",
        "NOT" => "NOT",
        "IN" => "IN",
        "LIKE" | "ILIKE" => "LIKE",
        "IS" => "IS",
        "NULL" => "NULL",
        _ => return None,
    })
}

fn tokenize(s: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            c if c.is_whitespace() => i += 1,
            '"' => {
                let (text, next) = quoted(&chars, i, '"')?;
                out.push(Token::Field(text));
                i = next;
            }
            '\'' => {
                let (text, next) = quoted(&chars, i, '\'')?;
                out.push(Token::Text(text));
                i = next;
            }
            '(' | ')' | ',' => {
                out.push(Token::Word(c.to_string()));
                i += 1;
            }
            '=' => {
                out.push(Token::Word("=".into()));
                i += 1;
            }
            '<' | '>' | '!' => {
                let two: String = chars.get(i..(i + 2).min(chars.len())).unwrap_or_default().iter().collect();
                let op = match two.as_str() {
                    "<=" | ">=" | "<>" | "!=" => two,
                    _ if c != '!' => c.to_string(),
                    _ => return None,
                };
                i += op.chars().count();
                out.push(Token::Word(op));
            }
            c if c.is_ascii_digit() || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) => {
                let start = i;
                i += 1;
                while chars.get(i).is_some_and(|d| d.is_ascii_digit() || *d == '.') {
                    i += 1;
                }
                // An exponent: 1e3, 2.5E-4.
                if chars.get(i).is_some_and(|e| *e == 'e' || *e == 'E') {
                    let sign = usize::from(chars.get(i + 1).is_some_and(|s| *s == '-' || *s == '+'));
                    if chars.get(i + 1 + sign).is_some_and(char::is_ascii_digit) {
                        i += 1 + sign;
                        while chars.get(i).is_some_and(char::is_ascii_digit) {
                            i += 1;
                        }
                    }
                }
                let n: String = chars.get(start..i).unwrap_or_default().iter().collect();
                n.parse::<f64>().ok()?;
                out.push(Token::Word(n));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while chars.get(i).is_some_and(|d| d.is_alphanumeric() || *d == '_') {
                    i += 1;
                }
                let w: String = chars.get(start..i).unwrap_or_default().iter().collect();
                // A word followed by a bracket is a function call.
                let next = chars.get(i..).unwrap_or_default().iter().find(|c| !c.is_whitespace());
                match keyword(&w) {
                    Some(k) => out.push(Token::Word(k.into())),
                    None if next == Some(&'(') => return None,
                    None => out.push(Token::Field(w)),
                }
            }
            // Arithmetic, concatenation, variables (@…) and the rest.
            _ => return None,
        }
        if out.len() > 2000 {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The text between the quote at `start` and its closing one (a doubled quote stands for
/// itself), and where reading goes on.
fn quoted(chars: &[char], start: usize, q: char) -> Option<(String, usize)> {
    let mut text = String::new();
    let mut i = start + 1;
    loop {
        match chars.get(i) {
            None => return None,
            Some(&c) if c == q && chars.get(i + 1) == Some(&q) => {
                text.push(q);
                i += 2;
            }
            Some(&c) if c == q => return Some((text, i + 1)),
            Some(&c) => {
                text.push(c);
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::to_sql;

    #[test]
    fn filters_carry_over_and_the_rest_is_refused() {
        assert_eq!(to_sql(r#""Symbol" = 'Ortsteil_20-49'"#).as_deref(), Some(r#""Symbol" = 'Ortsteil_20-49'"#));
        assert_eq!(
            to_sql(r#"pop >= 1e3 AND ("name" ILIKE 'z%' OR kind IN ('a', 'b''s')) AND note IS NOT NULL"#).as_deref(),
            Some(r#""pop" >= 1e3 AND ( "name" LIKE 'z%' OR "kind" IN ( 'a' , 'b''s' ) ) AND "note" IS NOT NULL"#)
        );
        assert_eq!(to_sql("a <> -2.5").as_deref(), Some(r#""a" <> -2.5"#));
        for refused in ["upper(\"name\") = 'X'", "\"a\" + 1 > 2", "\"a\" || 'x' = 'y'", "@map_scale < 5000", "'unclosed", "", "a ! b"] {
            assert_eq!(to_sql(refused), None, "{refused}");
        }
    }
}
