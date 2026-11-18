//! DBISAM-Scan: varre uma pasta de .dat e gera o schema.toml com a estrutura de cada tabela.
//!
//! Compatível com o formato antigo (name, field_type, offset, length) e com campos novos:
//! - `type_id`: tipo DBISAM original; `size`: bytes do valor no registro.
//! - field_type: S texto, I inteiro (length 1 = booleano, 2 = smallint, 4 = integer), F float, D data,
//!   T hora, TS data e hora, M memo/blob, L inteiro de 8 bytes, X bytes/BCD/desconhecido.
//!   (Quem lê o schema.toml antigo trata T, TS, M, L e X como texto.)

use dbisam::{Field, FieldKind, Table};
use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use walkdir::WalkDir;

#[derive(Serialize)]
struct Column {
    name: String,
    field_type: String,
    offset: u32,
    length: u32,
    type_id: u8,
    size: u32,
}

#[derive(Serialize)]
struct TableConfig {
    record_size: u32,
    columns: Vec<Column>,
}

fn column(f: &Field) -> Column {
    let (ft, length) = match f.kind {
        FieldKind::Text => ("S", f.declared_len as u32),
        FieldKind::Boolean => ("I", 1),
        FieldKind::SmallInt | FieldKind::Word => ("I", 2),
        FieldKind::Integer | FieldKind::AutoInc => ("I", 4),
        FieldKind::LargeInt => ("L", 8),
        FieldKind::Float => ("F", 8),
        FieldKind::Date => ("D", 4),
        FieldKind::Time => ("T", 4),
        FieldKind::Timestamp => ("TS", 8),
        FieldKind::Blob => ("M", f.size as u32),
        FieldKind::Bcd | FieldKind::Bytes | FieldKind::Unknown(_) => ("X", f.size as u32),
    };
    Column { name: f.name.clone(), field_type: ft.into(), offset: f.offset as u32, length, type_id: f.type_id, size: f.size as u32 }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            dotenvy::from_path(dir.join(".env")).ok();
        }
    }
    dotenvy::dotenv().ok();

    let base = env::var("DB_PATH").map(|v| v.replace('"', "")).unwrap_or_else(|_| {
        let cwd = env::current_dir().unwrap().to_string_lossy().to_string();
        println!("AVISO: DB_PATH não encontrado. Usando a pasta atual: {cwd}");
        cwd
    });

    let mut schema: BTreeMap<String, TableConfig> = BTreeMap::new();
    let mut unverified: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    let mut errors = 0;
    for entry in WalkDir::new(&base).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dat")) {
            continue;
        }
        match Table::open(path) {
            Ok(t) if !t.fields.is_empty() => {
                for f in &t.fields {
                    if !f.kind.verified() {
                        unverified.entry(f.type_id).or_default().push(format!("{}.{}", t.name, f.name));
                    }
                }
                println!("Tabela: {} ({} colunas, {} registros)", t.name, t.fields.len(), t.slots());
                schema.insert(
                    t.name.clone(),
                    TableConfig { record_size: t.record_size as u32, columns: t.fields.iter().map(column).collect() },
                );
            }
            Ok(_) => {}
            Err(e) => {
                errors += 1;
                eprintln!("ERRO: {}: {e}", path.display());
            }
        }
    }

    fs::write("schema.toml", toml::to_string_pretty(&schema)?)?;
    println!("\nPronto: {} tabelas em schema.toml ({errors} com erro).", schema.len());
    if !unverified.is_empty() {
        println!("AVISO: tipos ainda não conferidos em base real (mandar um exemplo para conferir):");
        for (id, cols) in &unverified {
            let shown: Vec<&String> = cols.iter().take(5).collect();
            println!("  tipo {id}: {} colunas, ex.: {:?}", cols.len(), shown);
        }
    }
    Ok(())
}
