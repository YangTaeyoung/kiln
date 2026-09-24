//! 선택 영역 복사 형식(TSV/CSV/JSON/SQL)과 파일 스트리밍 내보내기(CSV/JSON).

use crate::driver::ColumnInfo;
use crate::sql::quote_ident;
use crate::{Driver, Value};
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyFormat {
    Tsv,
    Csv,
    Json,
    SqlInsert,
}

impl CopyFormat {
    pub const ALL: [CopyFormat; 4] = [
        CopyFormat::Tsv,
        CopyFormat::Csv,
        CopyFormat::Json,
        CopyFormat::SqlInsert,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CopyFormat::Tsv => "TSV",
            CopyFormat::Csv => "CSV",
            CopyFormat::Json => "JSON",
            CopyFormat::SqlInsert => "SQL INSERT",
        }
    }
}

/// CSV 필드 인용.
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn tsv_field(s: &str) -> String {
    s.replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "")
}

/// 행들을 지정 형식 문자열로 만든다. `table` 은 SQL INSERT 대상 이름(인용 전).
pub fn format_rows(
    fmt: CopyFormat,
    driver: Driver,
    table: Option<&str>,
    cols: &[&ColumnInfo],
    rows: &[Vec<&Value>],
    with_header: bool,
) -> String {
    let mut out = String::new();
    match fmt {
        CopyFormat::Tsv | CopyFormat::Csv => {
            let (sep, field): (&str, fn(&str) -> String) = if fmt == CopyFormat::Tsv {
                ("\t", tsv_field)
            } else {
                (",", csv_field)
            };
            if with_header {
                out.push_str(
                    &cols
                        .iter()
                        .map(|c| field(&c.name))
                        .collect::<Vec<_>>()
                        .join(sep),
                );
                out.push('\n');
            }
            for r in rows {
                out.push_str(
                    &r.iter()
                        .map(|v| v.to_text().map(|t| field(&t)).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(sep),
                );
                out.push('\n');
            }
            if out.ends_with('\n') {
                out.pop();
            }
        }
        CopyFormat::Json => {
            let arr: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    let mut m = serde_json::Map::new();
                    for (c, v) in cols.iter().zip(r.iter()) {
                        m.insert(c.name.clone(), v.to_json());
                    }
                    serde_json::Value::Object(m)
                })
                .collect();
            out = serde_json::to_string_pretty(&arr).unwrap_or_default();
        }
        CopyFormat::SqlInsert => {
            let name = quote_ident(driver, table.unwrap_or("table_name"));
            let names = cols
                .iter()
                .map(|c| quote_ident(driver, &c.name))
                .collect::<Vec<_>>()
                .join(", ");
            for r in rows {
                let vals = r
                    .iter()
                    .map(|v| v.to_sql_literal(driver))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("INSERT INTO {name} ({names}) VALUES ({vals});\n"));
            }
            if out.ends_with('\n') {
                out.pop();
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
        }
    }
}

/// 행을 받는 대로 파일에 쓰는 내보내기 작성기.
pub struct ExportWriter {
    fmt: ExportFormat,
    out: std::io::BufWriter<std::fs::File>,
    header_written: bool,
    rows: u64,
    names: Vec<String>,
}

impl ExportWriter {
    pub fn create(path: &std::path::Path, fmt: ExportFormat) -> std::io::Result<ExportWriter> {
        let f = std::fs::File::create(path)?;
        Ok(ExportWriter {
            fmt,
            out: std::io::BufWriter::with_capacity(1 << 16, f),
            header_written: false,
            rows: 0,
            names: Vec::new(),
        })
    }

    pub fn write_row(&mut self, cols: &[ColumnInfo], row: &[Value]) -> std::io::Result<()> {
        if !self.header_written {
            self.header_written = true;
            self.names = cols.iter().map(|c| c.name.clone()).collect();
            match self.fmt {
                ExportFormat::Csv => {
                    let h = self
                        .names
                        .iter()
                        .map(|n| csv_field(n))
                        .collect::<Vec<_>>()
                        .join(",");
                    writeln!(self.out, "{h}")?;
                }
                ExportFormat::Json => self.out.write_all(b"[\n")?,
            }
        }
        match self.fmt {
            ExportFormat::Csv => {
                let line = row
                    .iter()
                    .map(|v| v.to_text().map(|t| csv_field(&t)).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(self.out, "{line}")?;
            }
            ExportFormat::Json => {
                let mut m = serde_json::Map::new();
                for (n, v) in self.names.iter().zip(row.iter()) {
                    m.insert(n.clone(), v.to_json());
                }
                if self.rows > 0 {
                    self.out.write_all(b",\n")?;
                }
                self.out.write_all(b"  ")?;
                serde_json::to_writer(&mut self.out, &serde_json::Value::Object(m))?;
            }
        }
        self.rows += 1;
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<u64> {
        match self.fmt {
            ExportFormat::Json => {
                if !self.header_written {
                    self.out.write_all(b"[")?;
                }
                self.out.write_all(b"\n]\n")?;
            }
            ExportFormat::Csv => {}
        }
        self.out.flush()?;
        Ok(self.rows)
    }
}
