//! Minimal strict JSON parser for offline proof bundles.
//!
//! The verifier intentionally has no third-party parser dependency. Authoritative numeric values
//! in proof bundles are canonical decimal strings; JSON numeric tokens are accepted only as
//! integral syntax and are never converted through floating point.

use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Integer(String),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct JsonError {
    pub offset: usize,
    pub kind: JsonErrorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsonErrorKind {
    UnexpectedEnd,
    UnexpectedToken,
    InvalidString,
    InvalidEscape,
    InvalidUnicode,
    InvalidNumber,
    DuplicateKey,
    TrailingData,
}

pub(crate) fn parse(input: &[u8]) -> Result<JsonValue, JsonError> {
    let mut parser = Parser { input, offset: 0 };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.offset != input.len() {
        return Err(parser.error(JsonErrorKind::TrailingData));
    }
    Ok(value)
}

struct Parser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl Parser<'_> {
    fn error(&self, kind: JsonErrorKind) -> JsonError {
        JsonError {
            offset: self.offset,
            kind,
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    fn take(&mut self) -> Result<u8, JsonError> {
        let byte = self.peek().ok_or_else(|| self.error(JsonErrorKind::UnexpectedEnd))?;
        self.offset += 1;
        Ok(byte)
    }

    fn expect_literal(&mut self, suffix: &[u8]) -> Result<(), JsonError> {
        let end = self
            .offset
            .checked_add(suffix.len())
            .ok_or_else(|| self.error(JsonErrorKind::UnexpectedEnd))?;
        if self.input.get(self.offset..end) != Some(suffix) {
            return Err(self.error(JsonErrorKind::UnexpectedToken));
        }
        self.offset = end;
        Ok(())
    }

    fn value(&mut self) -> Result<JsonValue, JsonError> {
        match self.peek() {
            Some(b'n') => {
                self.offset += 1;
                self.expect_literal(b"ull")?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.offset += 1;
                self.expect_literal(b"rue")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.offset += 1;
                self.expect_literal(b"alse")?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'"') => self.string().map(JsonValue::String),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-' | b'0'..=b'9') => self.integer().map(JsonValue::Integer),
            Some(_) => Err(self.error(JsonErrorKind::UnexpectedToken)),
            None => Err(self.error(JsonErrorKind::UnexpectedEnd)),
        }
    }

    fn array(&mut self) -> Result<JsonValue, JsonError> {
        self.take()?;
        self.skip_ws();
        let mut values = Vec::new();
        if self.peek() == Some(b']') {
            self.offset += 1;
            return Ok(JsonValue::Array(values));
        }
        loop {
            self.skip_ws();
            values.push(self.value()?);
            self.skip_ws();
            match self.take()? {
                b',' => {}
                b']' => return Ok(JsonValue::Array(values)),
                _ => return Err(self.error(JsonErrorKind::UnexpectedToken)),
            }
        }
    }

    fn object(&mut self) -> Result<JsonValue, JsonError> {
        self.take()?;
        self.skip_ws();
        let mut fields = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Ok(JsonValue::Object(fields));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error(JsonErrorKind::UnexpectedToken));
            }
            let key = self.string()?;
            self.skip_ws();
            if self.take()? != b':' {
                return Err(self.error(JsonErrorKind::UnexpectedToken));
            }
            self.skip_ws();
            let value = self.value()?;
            if fields.insert(key, value).is_some() {
                return Err(self.error(JsonErrorKind::DuplicateKey));
            }
            self.skip_ws();
            match self.take()? {
                b',' => {}
                b'}' => return Ok(JsonValue::Object(fields)),
                _ => return Err(self.error(JsonErrorKind::UnexpectedToken)),
            }
        }
    }

    fn integer(&mut self) -> Result<String, JsonError> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error(JsonErrorKind::InvalidNumber));
                }
            }
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.offset += 1;
                }
            }
            _ => return Err(self.error(JsonErrorKind::InvalidNumber)),
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err(self.error(JsonErrorKind::InvalidNumber));
        }
        let raw = std::str::from_utf8(&self.input[start..self.offset])
            .map_err(|_| self.error(JsonErrorKind::InvalidNumber))?;
        if raw == "-0" {
            return Err(self.error(JsonErrorKind::InvalidNumber));
        }
        Ok(raw.to_owned())
    }

    fn string(&mut self) -> Result<String, JsonError> {
        if self.take()? != b'"' {
            return Err(self.error(JsonErrorKind::UnexpectedToken));
        }
        let mut output = String::new();
        let mut literal_start = self.offset;
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error(JsonErrorKind::UnexpectedEnd));
            };
            match byte {
                b'"' => {
                    self.push_utf8_slice(&mut output, literal_start, self.offset)?;
                    self.offset += 1;
                    return Ok(output);
                }
                b'\\' => {
                    self.push_utf8_slice(&mut output, literal_start, self.offset)?;
                    self.offset += 1;
                    self.escape(&mut output)?;
                    literal_start = self.offset;
                }
                0x00..=0x1f => return Err(self.error(JsonErrorKind::InvalidString)),
                _ => self.offset += 1,
            }
        }
    }

    fn push_utf8_slice(
        &self,
        output: &mut String,
        start: usize,
        end: usize,
    ) -> Result<(), JsonError> {
        let text = std::str::from_utf8(&self.input[start..end]).map_err(|_| JsonError {
            offset: start,
            kind: JsonErrorKind::InvalidString,
        })?;
        output.push_str(text);
        Ok(())
    }

    fn escape(&mut self, output: &mut String) -> Result<(), JsonError> {
        match self.take()? {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{0008}'),
            b'f' => output.push('\u{000c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => {
                let first = self.hex4()?;
                if (0xd800..=0xdbff).contains(&first) {
                    if self.take()? != b'\\' || self.take()? != b'u' {
                        return Err(self.error(JsonErrorKind::InvalidUnicode));
                    }
                    let second = self.hex4()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(self.error(JsonErrorKind::InvalidUnicode));
                    }
                    let high = u32::from(first - 0xd800);
                    let low = u32::from(second - 0xdc00);
                    let scalar = 0x1_0000 + (high << 10) + low;
                    output.push(
                        char::from_u32(scalar)
                            .ok_or_else(|| self.error(JsonErrorKind::InvalidUnicode))?,
                    );
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(self.error(JsonErrorKind::InvalidUnicode));
                } else {
                    output.push(
                        char::from_u32(u32::from(first))
                            .ok_or_else(|| self.error(JsonErrorKind::InvalidUnicode))?,
                    );
                }
            }
            _ => return Err(self.error(JsonErrorKind::InvalidEscape)),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u16, JsonError> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let digit = match self.take()? {
                b'0'..=b'9' => self.input[self.offset - 1] - b'0',
                b'a'..=b'f' => self.input[self.offset - 1] - b'a' + 10,
                b'A'..=b'F' => self.input[self.offset - 1] - b'A' + 10,
                _ => return Err(self.error(JsonErrorKind::InvalidUnicode)),
            };
            value = value
                .checked_mul(16)
                .and_then(|value| value.checked_add(u16::from(digit)))
                .ok_or_else(|| self.error(JsonErrorKind::InvalidUnicode))?;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_parser_handles_nested_json_and_unicode() {
        let parsed = parse(br#"{"a":[true,null,"x\u263a"],"n":-12}"#).unwrap();
        let JsonValue::Object(fields) = parsed else {
            panic!("expected object");
        };
        assert_eq!(fields.get("n"), Some(&JsonValue::Integer("-12".into())));
        assert_eq!(
            fields.get("a"),
            Some(&JsonValue::Array(vec![
                JsonValue::Bool(true),
                JsonValue::Null,
                JsonValue::String("x☺".into()),
            ]))
        );
    }

    #[test]
    fn strict_parser_rejects_float_duplicate_and_trailing_data() {
        assert_eq!(
            parse(br#"{"x":1.5}"#).unwrap_err().kind,
            JsonErrorKind::InvalidNumber
        );
        assert_eq!(
            parse(br#"{"x":1,"x":2}"#).unwrap_err().kind,
            JsonErrorKind::DuplicateKey
        );
        assert_eq!(
            parse(br#"{}x"#).unwrap_err().kind,
            JsonErrorKind::TrailingData
        );
    }
}
