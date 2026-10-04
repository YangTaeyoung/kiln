//! 드라이버 중립 값 표현과 타입 분류, 표시 문자열 변환, 사용자 입력 파싱.

use std::fmt::Write as _;

/// 셀 하나의 값. 날짜/시간 계열은 DB 가 내놓은 정규 텍스트를 그대로 담는다.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(#[serde(with = "float_bits")] f64),
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Json(String),
    Date(String),
    Time(String),
    DateTime(String),
    Timestamptz(String),
    Uuid(String),
    Array(String),
    Other(String),
}

/// 컬럼 타입 이름을 분류한 결과. 디코딩·표시·편집 파싱 규칙을 결정한다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TypeClass {
    Bool,
    Int,
    UInt,
    Float,
    Decimal,
    Text,
    Bytes,
    Json,
    Date,
    Time,
    DateTime,
    Timestamptz,
    Uuid,
    Array,
    Bit,
    Other,
}

impl TypeClass {
    /// 타입 이름(대소문자 무관, 괄호 인자 포함 가능)을 분류한다.
    pub fn from_type_name(name: &str) -> TypeClass {
        let lower = name.trim().to_ascii_lowercase();
        if lower.ends_with("[]") || lower.starts_with('_') || lower.starts_with("array") {
            return TypeClass::Array;
        }
        let unsigned = lower.contains("unsigned");
        let base: String = lower
            .split(['(', ' '])
            .next()
            .unwrap_or("")
            .trim_matches('"')
            .to_string();
        match base.as_str() {
            "bool" | "boolean" => TypeClass::Bool,
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "int2"
            | "int4" | "int8" | "serial" | "bigserial" | "smallserial" | "year" | "oid" => {
                if unsigned {
                    TypeClass::UInt
                } else {
                    TypeClass::Int
                }
            }
            "float" | "float4" | "float8" | "double" | "real" => TypeClass::Float,
            "numeric" | "decimal" | "dec" | "fixed" => TypeClass::Decimal,
            "json" | "jsonb" => TypeClass::Json,
            "bytea" | "blob" | "tinyblob" | "mediumblob" | "longblob" | "binary" | "varbinary" => {
                TypeClass::Bytes
            }
            "date" => TypeClass::Date,
            "time" | "timetz" => TypeClass::Time,
            "timestamptz" => TypeClass::Timestamptz,
            "timestamp" => {
                if lower.contains("with time zone") {
                    TypeClass::Timestamptz
                } else {
                    TypeClass::DateTime
                }
            }
            "datetime" | "smalldatetime" => TypeClass::DateTime,
            "uuid" => TypeClass::Uuid,
            "bit" | "varbit" => TypeClass::Bit,
            "interval" | "point" | "inet" | "cidr" | "macaddr" | "money" | "tsvector" | "xml" => {
                TypeClass::Other
            }
            "char" | "character" | "varchar" | "nvarchar" | "nchar" | "text" | "tinytext"
            | "mediumtext" | "longtext" | "string" | "name" | "citext" | "bpchar" | "clob"
            | "enum" | "set" | "\"char\"" => TypeClass::Text,
            "" => TypeClass::Other,
            _ => {
                if lower.contains("char") || lower.contains("text") || lower.contains("clob") {
                    TypeClass::Text
                } else if lower.contains("int") {
                    TypeClass::Int
                } else if lower.contains("real") || lower.contains("floa") || lower.contains("doub")
                {
                    TypeClass::Float
                } else {
                    TypeClass::Other
                }
            }
        }
    }

    /// 숫자형이면 참. 그리드에서 오른쪽 정렬에 쓴다.
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            TypeClass::Int | TypeClass::UInt | TypeClass::Float | TypeClass::Decimal
        )
    }
}

/// 표시 문자열 기본 최대 길이(문자 수).
pub const DISPLAY_MAX_CHARS: usize = 200;

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// DB 텍스트 표현을 분류에 맞춰 값으로 변환한다. 파싱 실패 시 텍스트로 남긴다.
    pub fn from_text(s: &str, class: TypeClass) -> Value {
        match class {
            TypeClass::Bool => match s {
                "t" | "true" | "TRUE" | "1" | "y" | "yes" | "on" => Value::Bool(true),
                "f" | "false" | "FALSE" | "0" | "n" | "no" | "off" => Value::Bool(false),
                _ => s
                    .parse::<i64>()
                    .map(Value::Int)
                    .unwrap_or_else(|_| Value::Text(s.to_string())),
            },
            TypeClass::Int => s
                .parse::<i64>()
                .map(Value::Int)
                .or_else(|_| s.parse::<u64>().map(Value::UInt))
                .unwrap_or_else(|_| Value::Decimal(s.to_string())),
            TypeClass::UInt => s
                .parse::<u64>()
                .map(Value::UInt)
                .unwrap_or_else(|_| Value::Decimal(s.to_string())),
            TypeClass::Float => s
                .parse::<f64>()
                .map(Value::Float)
                .unwrap_or_else(|_| Value::Other(s.to_string())),
            TypeClass::Decimal => Value::Decimal(s.to_string()),
            TypeClass::Text => Value::Text(s.to_string()),
            TypeClass::Bytes => {
                if let Some(h) = s.strip_prefix("\\x") {
                    hex::decode(h)
                        .map(Value::Bytes)
                        .unwrap_or_else(|_| Value::Bytes(s.as_bytes().to_vec()))
                } else {
                    Value::Bytes(s.as_bytes().to_vec())
                }
            }
            TypeClass::Json => Value::Json(s.to_string()),
            TypeClass::Date => Value::Date(s.to_string()),
            TypeClass::Time => Value::Time(s.to_string()),
            TypeClass::DateTime => Value::DateTime(s.to_string()),
            TypeClass::Timestamptz => Value::Timestamptz(s.to_string()),
            TypeClass::Uuid => Value::Uuid(s.to_string()),
            TypeClass::Array => Value::Array(s.to_string()),
            TypeClass::Bit | TypeClass::Other => Value::Other(s.to_string()),
        }
    }

    /// 바이트열을 분류에 맞춰 값으로 변환한다. UTF-8 이 아니면 바이트 값으로 둔다.
    pub fn from_bytes(b: &[u8], class: TypeClass) -> Value {
        if class == TypeClass::Bytes {
            return Value::Bytes(b.to_vec());
        }
        match std::str::from_utf8(b) {
            Ok(s) => Value::from_text(s, class),
            Err(_) => Value::Bytes(b.to_vec()),
        }
    }

    /// 전체 텍스트 표현. 복사·내보내기·값 뷰어에서 쓴다. NULL 은 `None`.
    pub fn to_text(&self) -> Option<String> {
        Some(match self {
            Value::Null => return None,
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::UInt(u) => u.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Bytes(b) => format!("0x{}", hex::encode_upper(b)),
            Value::Decimal(s)
            | Value::Text(s)
            | Value::Json(s)
            | Value::Date(s)
            | Value::Time(s)
            | Value::DateTime(s)
            | Value::Timestamptz(s)
            | Value::Uuid(s)
            | Value::Array(s)
            | Value::Other(s) => s.clone(),
        })
    }

    /// 그리드 셀용 한 줄 표시 문자열. 줄바꿈은 기호로 바꾸고 `max` 문자에서 자른다.
    pub fn display(&self, max: usize) -> String {
        match self {
            Value::Null => "<null>".to_string(),
            Value::Bytes(b) => {
                let shown = b.len().min(max / 2);
                let mut s = String::with_capacity(shown * 2 + 16);
                s.push_str("0x");
                for byte in &b[..shown] {
                    let _ = write!(s, "{byte:02X}");
                }
                if shown < b.len() {
                    let _ = write!(s, "… ({} bytes)", b.len());
                }
                s
            }
            Value::Text(t)
            | Value::Json(t)
            | Value::Array(t)
            | Value::Other(t)
            | Value::Decimal(t) => one_line(t, max),
            _ => {
                let t = self.to_text().unwrap_or_default();
                one_line(&t, max)
            }
        }
    }

    /// JSON 내보내기용 값.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::from(*i),
            Value::UInt(u) => J::from(*u),
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(J::Number)
                .unwrap_or_else(|| J::String(format_float(*f))),
            Value::Json(s) => serde_json::from_str(s).unwrap_or_else(|_| J::String(s.clone())),
            Value::Bytes(b) => J::String(format!("0x{}", hex::encode_upper(b))),
            other => J::String(other.to_text().unwrap_or_default()),
        }
    }

    /// SQL 리터럴 표기. 복사(SQL INSERT)용이며 실행 경로는 바인딩을 쓴다.
    pub fn to_sql_literal(&self, driver: crate::Driver) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Bool(b) => {
                if driver == crate::Driver::Postgres {
                    if *b { "TRUE".into() } else { "FALSE".into() }
                } else if *b {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Value::Int(i) => i.to_string(),
            Value::UInt(u) => u.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Decimal(d) if d.parse::<f64>().is_ok() => d.clone(),
            Value::Bytes(b) => match driver {
                crate::Driver::Postgres => format!("'\\x{}'::bytea", hex::encode(b)),
                crate::Driver::Sqlite => format!("X'{}'", hex::encode_upper(b)),
                _ => format!("0x{}", hex::encode_upper(b)),
            },
            other => crate::sql::quote_literal(driver, &other.to_text().unwrap_or_default()),
        }
    }

    /// 사용자가 셀에 입력한 문자열을 컬럼 분류에 맞춰 검증·변환한다.
    pub fn parse_input(input: &str, class: TypeClass) -> Result<Value, String> {
        let s = input.trim();
        match class {
            TypeClass::Bool => match s.to_ascii_lowercase().as_str() {
                "true" | "t" | "1" | "yes" | "y" | "on" => Ok(Value::Bool(true)),
                "false" | "f" | "0" | "no" | "n" | "off" => Ok(Value::Bool(false)),
                _ => Err(format!("'{s}' is not a boolean")),
            },
            TypeClass::Int => s
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| format!("'{s}' is not an integer")),
            TypeClass::UInt => s
                .parse::<u64>()
                .map(Value::UInt)
                .map_err(|_| format!("'{s}' is not an unsigned integer")),
            TypeClass::Float => s
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| format!("'{s}' is not a number")),
            TypeClass::Decimal => {
                let ok = !s.is_empty()
                    && s.trim_start_matches(['-', '+']).chars().all(|c| {
                        c.is_ascii_digit()
                            || c == '.'
                            || c == 'e'
                            || c == 'E'
                            || c == '-'
                            || c == '+'
                    })
                    && s.chars().filter(|c| *c == '.').count() <= 1;
                if ok || s.eq_ignore_ascii_case("nan") {
                    Ok(Value::Decimal(s.to_string()))
                } else {
                    Err(format!("'{s}' is not a decimal number"))
                }
            }
            TypeClass::Json => serde_json::from_str::<serde_json::Value>(input)
                .map(|_| Value::Json(input.to_string()))
                .map_err(|e| format!("invalid JSON: {e}")),
            TypeClass::Uuid => uuid::Uuid::parse_str(s)
                .map(|u| Value::Uuid(u.to_string()))
                .map_err(|e| format!("invalid UUID: {e}")),
            TypeClass::Bytes => {
                let h = s
                    .strip_prefix("0x")
                    .or_else(|| s.strip_prefix("0X"))
                    .or_else(|| s.strip_prefix("\\x"));
                match h {
                    Some(h) => hex::decode(h)
                        .map(Value::Bytes)
                        .map_err(|e| format!("invalid hex: {e}")),
                    None => Ok(Value::Bytes(input.as_bytes().to_vec())),
                }
            }
            TypeClass::Date => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(|_| Value::Date(s.to_string()))
                .map_err(|_| format!("'{s}' is not a date (YYYY-MM-DD)")),
            TypeClass::Time => Ok(Value::Time(s.to_string())),
            TypeClass::DateTime => {
                let ok = [
                    "%Y-%m-%d %H:%M:%S%.f",
                    "%Y-%m-%dT%H:%M:%S%.f",
                    "%Y-%m-%d %H:%M",
                ]
                .iter()
                .any(|f| chrono::NaiveDateTime::parse_from_str(s, f).is_ok())
                    || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok();
                if ok {
                    Ok(Value::DateTime(s.to_string()))
                } else {
                    Err(format!("'{s}' is not a datetime (YYYY-MM-DD HH:MM:SS)"))
                }
            }
            TypeClass::Timestamptz => Ok(Value::Timestamptz(s.to_string())),
            TypeClass::Array => Ok(Value::Array(input.to_string())),
            TypeClass::Bit => Ok(Value::Other(s.to_string())),
            TypeClass::Text => Ok(Value::Text(input.to_string())),
            TypeClass::Other => Ok(Value::Other(input.to_string())),
        }
    }
}

/// 부동소수 표시. 정수값이면 소수점을 붙이지 않는다.
pub fn format_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}

/// 줄바꿈·탭을 기호로 바꾼 한 줄 문자열을 `max` 문자까지 만든다.
pub fn one_line(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max + 4));
    for (n, c) in s.chars().enumerate() {
        if n >= max {
            out.push('…');
            break;
        }
        match c {
            '\n' => out.push('↵'),
            '\r' => {}
            '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// 바이트열 16진 덤프(오프셋, 16진, ASCII).
pub fn hex_dump(b: &[u8], max_bytes: usize) -> String {
    let mut out = String::new();
    for (i, chunk) in b[..b.len().min(max_bytes)].chunks(16).enumerate() {
        let _ = write!(out, "{:08x}  ", i * 16);
        for j in 0..16 {
            match chunk.get(j) {
                Some(v) => {
                    let _ = write!(out, "{v:02x} ");
                }
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        for v in chunk {
            out.push(if v.is_ascii_graphic() || *v == b' ' {
                *v as char
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    if b.len() > max_bytes {
        let _ = write!(out, "… {} more bytes", b.len() - max_bytes);
    }
    out
}

// Preserve NaN/infinity and signed zero in recovery JSON without lossy JSON numbers.
mod float_bits {
    use serde::{Deserialize, Serializer, Deserializer};
    pub fn serialize<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> { s.serialize_u64(value.to_bits()) }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> { u64::deserialize(d).map(f64::from_bits) }
}
