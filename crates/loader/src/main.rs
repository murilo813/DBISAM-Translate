//! DBISAM-Loader: extrai tabelas DBISAM (.dat) para CSV pronto para o COPY do PostgreSQL.
//!
//! A estrutura é lida do próprio .dat (não depende do schema.toml), então continua certa quando o
//! sistema muda a tabela. Configuração por variáveis de ambiente (ou .env ao lado do executável):
//! - `DB_PATH`: pasta dos .dat.
//! - `TARGET_TABLE`: uma tabela, várias separadas por vírgula, ou `*` para todas.
//! - `OUTPUT_DIR`: pasta dos CSV (padrão: pasta atual).
//! - `RECORD_META=1`: acrescenta as colunas `_record_id` e `_record_hash` (para replicação incremental).
//! - `VERIFY_HASH=0`: não confere o hash de cada registro (padrão: confere).
//!
//! - `BLOB_BINARY=1`: grava também blobs binários (imagens) em hexadecimal bytea (`\x...`); padrão: NULL.
//!
//! CSV: cabeçalho com os nomes das colunas; NULL = campo vazio sem aspas; texto sempre entre aspas
//! (texto vazio = ""); datas ISO; números com precisão total. Memos de texto são lidos do .blb ao
//! lado do .dat; sem o .blb (ou com o memo ilegível) saem NULL e entram na contagem do resumo.

use dbisam::{MemoError, Table, Value};
use rayon::prelude::*;
use std::env;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const CHUNK: usize = 20_000;

struct Options {
    record_meta: bool,
    verify_hash: bool,
    blob_binary: bool,
}

#[derive(Default)]
struct Stats {
    active: usize,
    deleted: usize,
    bad_hash: usize,
    memos_read: usize,
    memos_no_file: usize,
    memos_bad: usize,
    raw_values: usize,
}

impl Stats {
    fn add(&mut self, o: &Stats) {
        self.active += o.active;
        self.deleted += o.deleted;
        self.bad_hash += o.bad_hash;
        self.memos_read += o.memos_read;
        self.memos_no_file += o.memos_no_file;
        self.memos_bad += o.memos_bad;
        self.raw_values += o.raw_values;
    }
}

fn main() {
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            dotenvy::from_path(dir.join(".env")).ok();
        }
    }
    dotenvy::dotenv().ok();

    let base = PathBuf::from(env::var("DB_PATH").map(|v| v.replace('"', "")).unwrap_or_else(|_| r"C:\BmSoft\Bases\zecao".into()));
    let target = env::var("TARGET_TABLE").map(|v| v.replace('"', "")).unwrap_or_else(|_| "Pessoas".into());
    let out_dir = PathBuf::from(env::var("OUTPUT_DIR").unwrap_or_else(|_| ".".into()));
    let opts = Options {
        record_meta: env::var("RECORD_META").map(|v| v == "1").unwrap_or(false),
        verify_hash: env::var("VERIFY_HASH").map(|v| v != "0").unwrap_or(true),
        blob_binary: env::var("BLOB_BINARY").map(|v| v == "1").unwrap_or(false),
    };

    let tables: Vec<PathBuf> = if target.trim() == "*" {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&base)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dat")))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    } else {
        target.split(',').map(|t| base.join(format!("{}.dat", t.trim()))).collect()
    };
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("ERRO: não consegui criar {}: {e}", out_dir.display());
        std::process::exit(1);
    }

    let mut failures = 0;
    for path in &tables {
        if let Err(e) = extract(path, &out_dir, &opts) {
            eprintln!("ERRO em {}: {e}", path.display());
            failures += 1;
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}

fn extract(path: &Path, out_dir: &Path, opts: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let table = Table::open(path)?;
    let out_path = out_dir.join(format!("extracao_{}.csv", table.name.to_lowercase()));
    let mut out = BufWriter::with_capacity(8 << 20, File::create(&out_path)?);

    let mut header: Vec<String> = Vec::new();
    if opts.record_meta {
        header.push("_record_id".into());
        header.push("_record_hash".into());
    }
    header.extend(table.fields.iter().map(|f| f.name.clone()));
    writeln!(out, "{}", header.join(","))?;

    let slots = table.slots();
    let chunks: Vec<(usize, usize)> = (0..slots).step_by(CHUNK).map(|s| (s, (s + CHUNK).min(slots))).collect();
    let batch = rayon::current_num_threads().max(1) * 2;
    let mut stats = Stats::default();
    for group in chunks.chunks(batch) {
        let parts: Vec<(String, Stats)> = group.par_iter().map(|&(a, b)| render(&table, a, b, opts)).collect();
        for (text, s) in parts {
            out.write_all(text.as_bytes())?;
            stats.add(&s);
        }
    }
    out.flush()?;

    let unverified: Vec<String> = table
        .fields
        .iter()
        .filter(|f| !f.kind.verified())
        .map(|f| format!("{} (tipo {})", f.name, f.type_id))
        .collect();
    let mut notes = Vec::new();
    if opts.verify_hash {
        notes.push(format!("hash inválido {}", stats.bad_hash));
    }
    if stats.memos_read > 0 {
        notes.push(format!("memos lidos {}", stats.memos_read));
    }
    if stats.memos_no_file > 0 {
        notes.push(format!("memos sem o .blb (saíram NULL) {}", stats.memos_no_file));
    }
    if stats.memos_bad > 0 {
        notes.push(format!("memos ilegíveis (saíram NULL) {}", stats.memos_bad));
    }
    if table.trailing_bytes() > 0 {
        notes.push(format!("{} bytes de registro incompleto no fim", table.trailing_bytes()));
    }
    if stats.raw_values > 0 {
        notes.push(format!("{} valores em hexadecimal (tipo não reconhecido)", stats.raw_values));
    }
    println!(
        "{}: {} registros -> {} | {:.1} s | cabeçalho diz {}, posições no arquivo {}, apagados {}{}",
        table.name,
        stats.active,
        out_path.display(),
        started.elapsed().as_secs_f64(),
        table.header_rows,
        slots,
        stats.deleted,
        notes.iter().map(|n| format!(", {n}")).collect::<String>(),
    );
    if !unverified.is_empty() {
        println!("   AVISO: tipos ainda não conferidos em base real: {}", unverified.join(", "));
    }
    if stats.memos_bad > 0 {
        println!("   AVISO: memos ilegíveis costumam ser .blb copiado em outro momento que o .dat. Copie os dois juntos (ou use VSS).");
    }
    if opts.verify_hash && stats.bad_hash > 0 {
        println!("   AVISO: registros com hash inválido podem estar pela metade (arquivo em uso). Extraia de novo ou use uma cópia de sombra (VSS).");
    }
    Ok(())
}

fn render(table: &Table, from: usize, to: usize, opts: &Options) -> (String, Stats) {
    let mut s = String::with_capacity((to - from) * table.record_size / 2);
    let mut st = Stats::default();
    for slot in from..to {
        let r = table.record(slot);
        if !r.is_active() {
            st.deleted += 1;
            continue;
        }
        st.active += 1;
        if opts.verify_hash && !r.hash_ok() {
            st.bad_hash += 1;
        }
        let mut first = true;
        if opts.record_meta {
            let _ = write!(s, "{}", r.record_id());
            s.push(',');
            for b in r.stored_hash() {
                let _ = write!(s, "{b:02x}");
            }
            first = false;
        }
        for f in &table.fields {
            if !first {
                s.push(',');
            }
            first = false;
            let value = match r.value_resolved(f, opts.blob_binary) {
                Ok(v) => {
                    if matches!(r.value(f), Value::Blob(_)) && !matches!(v, Value::Null) {
                        st.memos_read += 1;
                    }
                    v
                }
                Err(MemoError::NoBlobFile) => {
                    st.memos_no_file += 1;
                    Value::Null
                }
                Err(_) => {
                    st.memos_bad += 1;
                    Value::Null
                }
            };
            match value {
                Value::Null => {}
                Value::Text(t) => {
                    s.push('"');
                    for ch in t.chars() {
                        if ch == '"' {
                            s.push('"');
                        }
                        s.push(ch);
                    }
                    s.push('"');
                }
                // Não deveria sobrar ponteiro sem resolver; por garantia sai NULL.
                Value::Blob(_) => {}
                v @ Value::Raw(_) => {
                    st.raw_values += 1;
                    let _ = write!(s, "{v}");
                }
                v => {
                    let _ = write!(s, "{v}");
                }
            }
        }
        s.push('\n');
    }
    (s, st)
}
