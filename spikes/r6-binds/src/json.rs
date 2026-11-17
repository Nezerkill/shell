//! Minimal JSON reader (no dependencies: the spike must build offline, and
//! `serde_json` is not in the local cargo registry cache).
//!
//! Objects keep their key order, so generated output is deterministic.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::Str(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Value::Number(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(pairs) => Some(pairs),
            _ => None,
        }
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser {
        bytes: text.as_bytes(),
        text,
        pos: 0,
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(p.err("trailing data after the top-level value"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> String {
        let consumed = &self.text[..self.pos.min(self.text.len())];
        let line = consumed.matches('\n').count() + 1;
        let col = consumed
            .rsplit('\n')
            .next()
            .map(|l| l.chars().count() + 1)
            .unwrap_or(1);
        format!("JSON {line}:{col}: {msg}")
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, want: u8) -> Result<(), String> {
        match self.bump() {
            Some(b) if b == want => Ok(()),
            Some(b) => Err(self.err(&format!("expected `{}`, got `{}`", want as char, b as char))),
            None => Err(self.err(&format!("expected `{}`, got end of input", want as char))),
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b) => Err(self.err(&format!("unexpected `{}`", b as char))),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes.get(self.pos..self.pos + word.len()) == Some(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.err(&format!("expected `{word}`")))
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.expect(b'{')?;
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(pairs));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.value()?;
            if pairs.iter().any(|(k, _)| *k == key) {
                return Err(self.err(&format!("duplicate key `{key}`")));
            }
            pairs.push((key, value));
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => return Ok(Value::Object(pairs)),
                Some(b) => return Err(self.err(&format!("expected `,` or `}}`, got `{}`", b as char))),
                None => return Err(self.err("unterminated object")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => return Ok(Value::Array(items)),
                Some(b) => return Err(self.err(&format!("expected `,` or `]`, got `{}`", b as char))),
                None => return Err(self.err("unterminated array")),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                Some(b'"') => return Ok(out),
                Some(b'\\') => match self.bump() {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'b') => out.push('\u{8}'),
                    Some(b'f') => out.push('\u{c}'),
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'u') => {
                        let hi = self.hex4()?;
                        let ch = if (0xD800..0xDC00).contains(&hi) {
                            if self.bump() != Some(b'\\') || self.bump() != Some(b'u') {
                                return Err(self.err("lone high surrogate in \\u escape"));
                            }
                            let lo = self.hex4()?;
                            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                        } else {
                            hi
                        };
                        out.push(char::from_u32(ch).unwrap_or('\u{fffd}'));
                    }
                    _ => return Err(self.err("invalid escape")),
                },
                Some(b) if b < 0x20 => return Err(self.err("raw control character in string")),
                Some(b) => {
                    // Re-decode from the original text so multi-byte UTF-8 survives.
                    let start = self.pos - 1;
                    let len = utf8_len(b);
                    let end = start + len;
                    let s = self
                        .text
                        .get(start..end)
                        .ok_or_else(|| self.err("truncated UTF-8 sequence"))?;
                    out.push_str(s);
                    self.pos = end;
                }
                None => return Err(self.err("unterminated string")),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut n = 0;
        for _ in 0..4 {
            let b = self.bump().ok_or_else(|| self.err("truncated \\u escape"))?;
            let d = (b as char)
                .to_digit(16)
                .ok_or_else(|| self.err("invalid hex digit"))?;
            n = n * 16 + d;
        }
        Ok(n)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
            self.pos += 1;
        }
        Ok(Value::Number(self.text[start..self.pos].to_string()))
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_and_keeps_order() {
        let v = parse(r#"{"b":{"y":1,"x":[true,null,"s"]},"a":2}"#).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj[0].0, "b");
        assert!(v.get("b").unwrap().get("y").unwrap().as_u32() == Some(1));
    }

    #[test]
    fn escapes_and_utf8() {
        let v = parse(r#"{"t":"Терминал \"q\" \\ \n"}"#).unwrap();
        assert_eq!(v.get("t").unwrap().as_str().unwrap(), "Терминал \"q\" \\ \n");
    }

    #[test]
    fn rejects_trailing_data() {
        assert!(parse(r#"{"a":1} x"#).is_err());
    }
}
