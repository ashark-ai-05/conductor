//! The assertion language a workflow uses in `command_assert` gates.
//!
//! A closed grammar, evaluated by the engine, so that deciding a gate never involves
//! interpreting prose (SPEC §6 rule 2):
//!
//! ```text
//! expr    := operand op operand
//!          | path ("all" | "any") op operand
//! operand := number | true | false | 'text' | name
//! op      := == | != | > | >= | < | <=
//! ```
//!
//! `failures.kind all == 'assertion'` reads: every record in the list `failures` has a
//! `kind` equal to `assertion`. An empty list satisfies `all` and fails `any`.
//!
//! An expression that can't be parsed, or that names a fact nobody supplied, is an error, not
//! a `false`. The gate that owns it is then unwitnessed rather than failed, because nothing
//! was actually checked.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    Num(f64),
    Bool(bool),
    Text(String),
    List(Vec<BTreeMap<String, Fact>>),
}

impl fmt::Display for Fact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fact::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => write!(f, "{}", *n as i64),
            Fact::Num(n) => write!(f, "{n}"),
            Fact::Bool(b) => write!(f, "{b}"),
            Fact::Text(s) => write!(f, "'{s}'"),
            Fact::List(l) => write!(f, "[{} items]", l.len()),
        }
    }
}

pub type Facts = BTreeMap<String, Fact>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssertError {
    #[error("`{expr}` is not an assertion conductor understands: {why}")]
    Parse { expr: String, why: String },
    #[error("`{expr}` names `{name}`, which this gate's output doesn't provide")]
    UnknownFact { expr: String, name: String },
    #[error("`{expr}` compares {left} with {right}, which can't be compared")]
    Mismatch {
        expr: String,
        left: String,
        right: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "==" => Op::Eq,
            "!=" => Op::Ne,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "<" => Op::Lt,
            "<=" => Op::Le,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Operand {
    Lit(Fact),
    Name(String),
}

fn tokens(expr: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = expr.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '\'' {
            chars.next();
            let mut s = String::from("'");
            loop {
                match chars.next() {
                    Some('\'') => break,
                    Some(ch) => s.push(ch),
                    None => return Err("a quoted text is never closed".into()),
                }
            }
            s.push('\'');
            out.push(s);
        } else if "=!<>".contains(c) {
            let mut s = String::new();
            while let Some(&d) = chars.peek() {
                if "=!<>".contains(d) {
                    s.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push(s);
        } else {
            let mut s = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_whitespace() || "=!<>'".contains(d) {
                    break;
                }
                s.push(d);
                chars.next();
            }
            out.push(s);
        }
    }
    Ok(out)
}

fn operand(tok: &str) -> Result<Operand, String> {
    if let Some(t) = tok.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        return Ok(Operand::Lit(Fact::Text(t.to_owned())));
    }
    match tok {
        "true" => return Ok(Operand::Lit(Fact::Bool(true))),
        "false" => return Ok(Operand::Lit(Fact::Bool(false))),
        _ => {}
    }
    if let Ok(n) = tok.parse::<f64>()
        && n.is_finite()
    {
        return Ok(Operand::Lit(Fact::Num(n)));
    }
    let ident = |s: &str| {
        !s.is_empty()
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !s.starts_with(|c: char| c.is_ascii_digit())
    };
    if tok.split('.').all(ident) {
        Ok(Operand::Name(tok.to_owned()))
    } else {
        Err(format!("`{tok}` is neither a value nor a name"))
    }
}

/// What an assertion evaluated to, with the value it read, for the receipt.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    pub expr: String,
    pub holds: bool,
    /// The left-hand value actually compared, rendered for a person.
    pub observed: String,
}

pub fn evaluate(expr: &str, facts: &Facts) -> Result<Evaluated, AssertError> {
    let parse_err = |why: String| AssertError::Parse {
        expr: expr.to_owned(),
        why,
    };
    let owned = tokens(expr).map_err(parse_err)?;
    let toks: Vec<&str> = owned.iter().map(String::as_str).collect();
    match toks.as_slice() {
        [l, op, r] => {
            let op =
                Op::parse(op).ok_or_else(|| parse_err(format!("`{op}` is not a comparison")))?;
            let left = resolve(expr, &operand(l).map_err(parse_err)?, facts)?;
            let right = resolve(expr, &operand(r).map_err(parse_err)?, facts)?;
            let holds = compare(expr, &left, op, &right)?;
            Ok(Evaluated {
                expr: expr.to_owned(),
                holds,
                observed: left.to_string(),
            })
        }
        [path, quant @ ("all" | "any"), op, r] => {
            let op =
                Op::parse(op).ok_or_else(|| parse_err(format!("`{op}` is not a comparison")))?;
            let (list, field) = path.rsplit_once('.').ok_or_else(|| {
                parse_err(format!(
                    "`{path}` must be a list and a field, such as `failures.kind`"
                ))
            })?;
            let right = resolve(expr, &operand(r).map_err(parse_err)?, facts)?;
            let records = match facts.get(list) {
                Some(Fact::List(records)) => records,
                Some(other) => {
                    return Err(AssertError::Mismatch {
                        expr: expr.into(),
                        left: other.to_string(),
                        right: "a list".into(),
                    });
                }
                None => {
                    return Err(AssertError::UnknownFact {
                        expr: expr.into(),
                        name: list.into(),
                    });
                }
            };
            let mut results = Vec::with_capacity(records.len());
            let mut seen = Vec::new();
            for rec in records {
                let v = rec.get(field).ok_or_else(|| AssertError::UnknownFact {
                    expr: expr.into(),
                    name: format!("{list}.{field}"),
                })?;
                results.push(compare(expr, v, op, &right)?);
                seen.push(v.to_string());
            }
            let holds = if *quant == "all" {
                results.iter().all(|b| *b)
            } else {
                results.iter().any(|b| *b)
            };
            seen.sort();
            seen.dedup();
            Ok(Evaluated {
                expr: expr.to_owned(),
                holds,
                observed: format!("{} of {}: {}", records.len(), list, seen.join(", ")),
            })
        }
        _ => Err(parse_err(
            "expected `left op right` or `list.field all|any op value`".into(),
        )),
    }
}

fn resolve(expr: &str, o: &Operand, facts: &Facts) -> Result<Fact, AssertError> {
    match o {
        Operand::Lit(f) => Ok(f.clone()),
        Operand::Name(n) => facts
            .get(n)
            .cloned()
            .ok_or_else(|| AssertError::UnknownFact {
                expr: expr.into(),
                name: n.clone(),
            }),
    }
}

fn compare(expr: &str, l: &Fact, op: Op, r: &Fact) -> Result<bool, AssertError> {
    use std::cmp::Ordering;
    let ord = match (l, r) {
        (Fact::Num(a), Fact::Num(b)) => a.partial_cmp(b),
        (Fact::Text(a), Fact::Text(b)) => Some(a.cmp(b)),
        (Fact::Bool(a), Fact::Bool(b)) if matches!(op, Op::Eq | Op::Ne) => Some(a.cmp(b)),
        _ => None,
    };
    let Some(ord) = ord else {
        return Err(AssertError::Mismatch {
            expr: expr.into(),
            left: l.to_string(),
            right: r.to_string(),
        });
    };
    Ok(match op {
        Op::Eq => ord == Ordering::Equal,
        Op::Ne => ord != Ordering::Equal,
        Op::Gt => ord == Ordering::Greater,
        Op::Ge => ord != Ordering::Less,
        Op::Lt => ord == Ordering::Less,
        Op::Le => ord != Ordering::Greater,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        let rec = |k: &str| BTreeMap::from([("kind".to_string(), Fact::Text(k.into()))]);
        Facts::from([
            ("compiled".into(), Fact::Bool(true)),
            ("tests_run".into(), Fact::Num(7.0)),
            ("tests_failed".into(), Fact::Num(7.0)),
            ("tests_new".into(), Fact::Num(7.0)),
            (
                "failures".into(),
                Fact::List(vec![
                    rec("assertion"),
                    rec("assertion"),
                    rec("unimplemented"),
                ]),
            ),
            ("nothing".into(), Fact::List(vec![])),
        ])
    }

    fn holds(e: &str) -> bool {
        evaluate(e, &facts())
            .unwrap_or_else(|err| panic!("{err}"))
            .holds
    }

    #[test]
    fn comparisons_between_facts_and_values() {
        assert!(holds("compiled == true"));
        assert!(holds("tests_run > 0"));
        assert!(holds("tests_failed == tests_new"));
        assert!(!holds("tests_failed == 0"));
        assert!(holds("tests_run >= 7"));
        assert!(!holds("tests_run < 7"));
    }

    #[test]
    fn quantifiers_over_a_list() {
        assert!(!holds("failures.kind all == 'assertion'"));
        assert!(holds("failures.kind any == 'unimplemented'"));
        assert!(holds("failures.kind all != 'trivial'"));
    }

    #[test]
    fn an_empty_list_satisfies_all_and_fails_any() {
        let mut f = facts();
        f.insert("nothing".into(), Fact::List(vec![]));
        assert!(evaluate("nothing.kind all == 'x'", &f).unwrap().holds);
        assert!(!evaluate("nothing.kind any == 'x'", &f).unwrap().holds);
    }

    #[test]
    fn the_observed_value_is_kept_for_the_receipt() {
        let e = evaluate("tests_failed == 0", &facts()).unwrap();
        assert_eq!(e.observed, "7");
        let e = evaluate("failures.kind all == 'assertion'", &facts()).unwrap();
        assert_eq!(e.observed, "3 of failures: 'assertion', 'unimplemented'");
    }

    #[test]
    fn an_unknown_name_is_an_error_not_false() {
        assert!(matches!(
            evaluate("coverage > 80", &facts()),
            Err(AssertError::UnknownFact { .. })
        ));
    }

    #[test]
    fn prose_is_rejected() {
        assert!(matches!(
            evaluate("max > 480000 sustained 11m", &facts()),
            Err(AssertError::Parse { .. })
        ));
        assert!(matches!(
            evaluate("the tests look good", &facts()),
            Err(AssertError::Parse { .. })
        ));
        assert!(matches!(
            evaluate("tests_run => 1", &facts()),
            Err(AssertError::Parse { .. })
        ));
    }

    #[test]
    fn mismatched_types_are_an_error() {
        assert!(matches!(
            evaluate("compiled > 1", &facts()),
            Err(AssertError::Mismatch { .. })
        ));
    }
}
