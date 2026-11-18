//! Leitura direta de tabelas DBISAM 4 (arquivos `.dat`), sem driver.
//!
//! Formato (conferido em bases reais do BM, nov/2026):
//! - Cabeçalho de 512 bytes: `0x29` total de registros (u32; pode ficar atrás quando o arquivo é copiado
//!   com o sistema gravando), `0x2D` tamanho do registro (u16), `0x2F` número de campos (u16).
//! - Um descritor de 768 bytes por campo a partir de `0x200`: `[2]` tamanho do nome, `[3..]` nome,
//!   `[0xA4]` tipo, `[0xA5]` subtipo, `[0xA6]` tamanho declarado (u16), `[0xA9]` bytes no registro (u16),
//!   `[0xAC]` posição no registro (u16).
//! - Os registros começam em `0x200 + campos * 768`, todos do mesmo tamanho.
//! - Cada registro: `[0]` situação (0 = ativo), `[5..9]` RecordID (u32), `[9..25]` RecordHash
//!   (MD5 de `registro[25..]` com os campos memo zerados), e depois os campos.
//! - Cada campo: 1 byte de nulo (0 = NULL, 1 = tem valor) seguido do valor.
//!
//! Memos (arquivo `.blb` ao lado do `.dat`, conferido em 3,1 milhões de memos):
//! - Tamanho do bloco = 256 × `.dat[0x34]` (256 ou 512 nas bases vistas). Cabeçalho de 512 bytes
//!   (`0x1D` = total de blocos); o bloco de índice `i` (a partir de 0) começa em `512 + i * bloco`.
//! - O campo memo no registro guarda dois u32: o primeiro aponta o bloco inicial, contado em
//!   unidades de 256 bytes a partir de 1: índice = teto((ponteiro − 1) / (bloco / 256)).
//! - Cada trecho começa com 18 bytes: anterior (u32, 0 no primeiro), próximo (u32, mesma contagem do
//!   ponteiro; 0 = fim), bytes de dados do trecho (u16, contínuos, podem atravessar blocos),
//!   id (u32) e tamanho total do memo (u32, só no primeiro trecho). Texto em Windows-1252.

use chrono::{Duration, NaiveDate, NaiveDateTime};
use memmap2::Mmap;
use std::fmt;
use std::fs::File;
use std::path::Path;

pub const HEADER_SIZE: usize = 0x200;
pub const FIELD_DESC_SIZE: usize = 768;
pub const RECORD_HEADER_SIZE: usize = 25;

/// Tipos de campo do DBISAM 4. Os marcados "visto" foram conferidos em bases reais; os outros seguem
/// a numeração do DBISAM e são decodificados pelo tamanho, com aviso no scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,      // 1 (visto)
    Date,      // 2 (visto): dias desde 01/01/0001 (ordinal)
    Blob,      // 3 (visto): ponteiro de 8 bytes para o .blb (subtipo 22 memo, 26 imagem)
    Boolean,   // 4 (visto): 2 bytes
    SmallInt,  // 5
    Integer,   // 6 (visto; subtipo 29 = autoincremento)
    Float,     // 7 (visto; subtipo 21 = moeda)
    Bcd,       // 8
    Bytes,     // 9
    Time,      // 10 (visto): milissegundos desde a meia-noite
    Timestamp, // 11 (visto): double em milissegundos; ms / 86.400.000 = ordinal do dia (01/01/0001 = 1)
    Word,      // 12
    LargeInt,  // 13
    AutoInc,   // 14
    Unknown(u8),
}

impl FieldKind {
    pub fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Text,
            2 => Self::Date,
            3 => Self::Blob,
            4 => Self::Boolean,
            5 => Self::SmallInt,
            6 => Self::Integer,
            7 => Self::Float,
            8 => Self::Bcd,
            9 => Self::Bytes,
            10 => Self::Time,
            11 => Self::Timestamp,
            12 => Self::Word,
            13 => Self::LargeInt,
            14 => Self::AutoInc,
            n => Self::Unknown(n),
        }
    }

    /// Tipo já conferido em base real.
    pub fn verified(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Date | Self::Blob | Self::Boolean | Self::Integer | Self::Float | Self::Time | Self::Timestamp
        )
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub type_id: u8,
    pub subtype: u8,
    pub kind: FieldKind,
    /// Tamanho declarado (texto: número de caracteres).
    pub declared_len: u16,
    /// Bytes do valor dentro do registro (sem o byte de nulo).
    pub size: u16,
    /// Posição do byte de nulo dentro do registro; o valor começa em `offset + 1`.
    pub offset: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    Null,
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Date(NaiveDate),
    Time(u32),
    Timestamp(NaiveDateTime),
    /// Ponteiro para o arquivo .blb (conteúdo do memo não resolvido).
    Blob(&'a [u8]),
    /// Conteúdo binário de um blob que não é texto (ex.: imagem).
    Bytes(Vec<u8>),
    Raw(&'a [u8]),
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Format(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Format(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub struct Table {
    pub name: String,
    mmap: Mmap,
    /// Total de registros segundo o cabeçalho (informativo; não usar como limite).
    pub header_rows: u32,
    pub record_size: usize,
    pub data_offset: usize,
    /// Campos na ordem física do registro.
    pub fields: Vec<Field>,
    /// Faixas (início, tamanho com o byte de nulo) dos campos memo, zeradas no cálculo do RecordHash.
    blob_spans: Vec<(usize, usize)>,
    /// Arquivo .blb (memos), se existir ao lado do .dat.
    blb: Option<Mmap>,
    /// Tamanho do bloco do .blb.
    pub blob_block_size: usize,
}

/// Problema ao ler um memo do .blb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoError {
    /// Não existe .blb ao lado do .dat.
    NoBlobFile,
    /// Ponteiro aponta para fora do arquivo (.blb mais antigo que o .dat?).
    OutOfRange,
    /// Primeiro trecho inválido (memo regravado depois da cópia?).
    BadHeader,
    /// Encadeamento terminou antes do tamanho total.
    BrokenChain,
}

impl Table {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let file = File::open(path)?;
        // Somente leitura: o arquivo pode estar em uso pelo sistema.
        let mmap = unsafe { Mmap::map(&file)? };
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut table = Self::from_mmap(name, mmap)?;
        for ext in ["blb", "BLB", "Blb"] {
            let p = path.with_extension(ext);
            if let Ok(f) = File::open(&p) {
                table.blb = Some(unsafe { Mmap::map(&f)? });
                break;
            }
        }
        Ok(table)
    }

    fn from_mmap(name: String, mmap: Mmap) -> Result<Self, Error> {
        if mmap.len() < HEADER_SIZE {
            return Err(Error::Format(format!("{name}: arquivo menor que o cabeçalho")));
        }
        let header_rows = u32_at(&mmap, 0x29);
        let record_size = u16_at(&mmap, 0x2D) as usize;
        let field_count = u16_at(&mmap, 0x2F) as usize;
        let data_offset = HEADER_SIZE + field_count * FIELD_DESC_SIZE;
        if record_size <= RECORD_HEADER_SIZE || mmap.len() < data_offset {
            return Err(Error::Format(format!("{name}: cabeçalho inválido (registro {record_size}, campos {field_count})")));
        }
        let mut fields = Vec::with_capacity(field_count);
        for i in 0..field_count {
            let d = &mmap[HEADER_SIZE + i * FIELD_DESC_SIZE..HEADER_SIZE + (i + 1) * FIELD_DESC_SIZE];
            let name_len = d[2] as usize;
            let type_id = d[0xA4];
            if name_len == 0 || name_len > 64 || type_id == 0 {
                continue;
            }
            let fname = String::from_utf8_lossy(&d[3..3 + name_len]).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
            fields.push(Field {
                name: fname,
                type_id,
                subtype: d[0xA5],
                kind: FieldKind::from_id(type_id),
                declared_len: u16_at(d, 0xA6),
                size: u16_at(d, 0xA9),
                offset: u16_at(d, 0xAC),
            });
        }
        fields.sort_by_key(|f| f.offset);
        for f in &fields {
            let end = f.offset as usize + 1 + f.size as usize;
            if (f.offset as usize) < RECORD_HEADER_SIZE || end > record_size {
                return Err(Error::Format(format!(
                    "{name}: campo {} fora do registro (posição {}, {} bytes, registro {record_size})",
                    f.name, f.offset, f.size
                )));
            }
        }
        let blob_spans = fields
            .iter()
            .filter(|f| f.kind == FieldKind::Blob)
            .map(|f| (f.offset as usize, 1 + f.size as usize))
            .collect();
        let blob_block_size = 256 * (mmap[0x34].max(1) as usize);
        Ok(Self { name, mmap, header_rows, record_size, data_offset, fields, blob_spans, blb: None, blob_block_size })
    }

    pub fn has_blob_file(&self) -> bool {
        self.blb.is_some()
    }

    /// Posição no .blb do bloco apontado (ponteiros contam unidades de 256 bytes a partir de 1).
    fn blob_offset(&self, pointer: u32) -> Option<usize> {
        if pointer == 0 {
            return None;
        }
        let k = self.blob_block_size / 256;
        let index = (pointer as usize - 1).div_ceil(k);
        Some(HEADER_SIZE + index * self.blob_block_size)
    }

    /// Lê o conteúdo de um memo a partir do ponteiro de 8 bytes gravado no registro.
    pub fn read_blob(&self, pointer: &[u8]) -> Result<Vec<u8>, MemoError> {
        let blb = self.blb.as_ref().ok_or(MemoError::NoBlobFile)?;
        let first = u32_at(pointer, 0);
        let mut at = self.blob_offset(first).ok_or(MemoError::OutOfRange)?;
        if at + 18 > blb.len() {
            return Err(MemoError::OutOfRange);
        }
        let total = u32_at(blb, at + 14) as usize;
        if u32_at(blb, at) != 0 || total == 0 || u16_at(blb, at + 8) == 0 {
            return Err(MemoError::BadHeader);
        }
        let mut data = Vec::with_capacity(total);
        let mut hops = 0;
        loop {
            let used = u16_at(blb, at + 8) as usize;
            let take = used.min(total - data.len());
            if used == 0 || at + 18 + take > blb.len() {
                return Err(MemoError::BrokenChain);
            }
            data.extend_from_slice(&blb[at + 18..at + 18 + take]);
            if data.len() >= total {
                return Ok(data);
            }
            hops += 1;
            let next = u32_at(blb, at + 4);
            at = match self.blob_offset(next) {
                Some(o) if o + 18 <= blb.len() && hops < 1_000_000 => o,
                _ => return Err(MemoError::BrokenChain),
            };
        }
    }

    /// Posições de registro no arquivo (ativos e apagados). Pode ser maior que `header_rows`.
    pub fn slots(&self) -> usize {
        (self.mmap.len() - self.data_offset) / self.record_size
    }

    /// Bytes que sobram no fim do arquivo (registro incompleto: arquivo sendo gravado/copiado).
    pub fn trailing_bytes(&self) -> usize {
        (self.mmap.len() - self.data_offset) % self.record_size
    }

    pub fn record(&self, slot: usize) -> Record<'_> {
        let start = self.data_offset + slot * self.record_size;
        Record { table: self, bytes: &self.mmap[start..start + self.record_size] }
    }

    /// Registros ativos (situação 0), em ordem física.
    pub fn records(&self) -> impl Iterator<Item = Record<'_>> {
        (0..self.slots()).map(|i| self.record(i)).filter(|r| r.is_active())
    }

    /// Assinatura da estrutura (muda quando o sistema altera a tabela: coluna nova, tamanho...).
    pub fn structure_signature(&self) -> String {
        let mut ctx = md5::Context::new();
        ctx.consume(self.record_size.to_le_bytes());
        for f in &self.fields {
            ctx.consume(f.name.as_bytes());
            ctx.consume([f.type_id, f.subtype]);
            ctx.consume(f.size.to_le_bytes());
            ctx.consume(f.offset.to_le_bytes());
        }
        format!("{:x}", ctx.compute())
    }
}

pub struct Record<'a> {
    table: &'a Table,
    pub bytes: &'a [u8],
}

impl<'a> Record<'a> {
    pub fn is_active(&self) -> bool {
        self.bytes[0] == 0
    }

    /// Identificador interno do registro no DBISAM (sobe a cada inserção).
    pub fn record_id(&self) -> u32 {
        u32_at(self.bytes, 5)
    }

    /// Hash gravado pelo DBISAM (MD5 dos dados sem os memos). Muda quando o registro é alterado.
    pub fn stored_hash(&self) -> &'a [u8] {
        &self.bytes[9..RECORD_HEADER_SIZE]
    }

    /// Recalcula o hash e compara com o gravado: falso indica registro pela metade (gravação em curso)
    /// ou formato diferente do esperado.
    pub fn hash_ok(&self) -> bool {
        let digest = if self.table.blob_spans.is_empty() {
            md5::compute(&self.bytes[RECORD_HEADER_SIZE..])
        } else {
            let mut data = self.bytes[RECORD_HEADER_SIZE..].to_vec();
            for &(start, len) in &self.table.blob_spans {
                data[start - RECORD_HEADER_SIZE..start - RECORD_HEADER_SIZE + len].fill(0);
            }
            md5::compute(&data)
        };
        digest.0 == self.stored_hash()
    }

    pub fn value(&self, field: &Field) -> Value<'a> {
        decode(self.bytes, field)
    }

    /// Como `value`, mas resolve os memos pelo .blb: memo de texto (subtipo 22) vira `Text`, outros
    /// blobs viram `Bytes` (só se `binary` for verdadeiro; senão `Null`). Em erro devolve o motivo.
    pub fn value_resolved(&self, field: &Field, binary: bool) -> Result<Value<'a>, MemoError> {
        match decode(self.bytes, field) {
            Value::Blob(ptr) => {
                if field.subtype == 22 {
                    let data = self.table.read_blob(ptr)?;
                    let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&data);
                    Ok(Value::Text(s.into_owned()))
                } else if binary {
                    Ok(Value::Bytes(self.table.read_blob(ptr)?))
                } else {
                    Ok(Value::Null)
                }
            }
            v => Ok(v),
        }
    }
}

/// Decodifica um campo de um registro.
pub fn decode<'a>(record: &'a [u8], f: &Field) -> Value<'a> {
    let o = f.offset as usize;
    if record[o] == 0 {
        return Value::Null;
    }
    let raw = &record[o + 1..o + 1 + f.size as usize];
    let int = |n: usize| -> Option<i64> {
        match (n, raw.len() >= n) {
            (2, true) => Some(i16::from_le_bytes([raw[0], raw[1]]) as i64),
            (4, true) => Some(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64),
            (8, true) => Some(i64::from_le_bytes(raw[..8].try_into().unwrap())),
            _ => None,
        }
    };
    match f.kind {
        FieldKind::Text => {
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&raw[..end]);
            Value::Text(s.into_owned())
        }
        FieldKind::Date => match int(4) {
            Some(d) if d > 0 => NaiveDate::from_num_days_from_ce_opt(d as i32).map(Value::Date).unwrap_or(Value::Raw(raw)),
            _ => Value::Null,
        },
        FieldKind::Time => match int(4) {
            Some(ms) if (0..86_400_000).contains(&ms) => Value::Time(ms as u32),
            _ => Value::Raw(raw),
        },
        FieldKind::Timestamp => {
            if raw.len() < 8 {
                return Value::Raw(raw);
            }
            let ms = f64::from_le_bytes(raw[..8].try_into().unwrap());
            if ms <= 0.0 {
                return Value::Null;
            }
            let base = NaiveDate::from_ymd_opt(1, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
            base.checked_add_signed(Duration::milliseconds(ms.round() as i64 - 86_400_000))
                .map(Value::Timestamp)
                .unwrap_or(Value::Raw(raw))
        }
        FieldKind::Boolean => Value::Bool(raw.iter().any(|&b| b != 0)),
        FieldKind::SmallInt => int(2).map(Value::Int).unwrap_or(Value::Raw(raw)),
        FieldKind::Integer | FieldKind::AutoInc => int(4).map(Value::Int).unwrap_or(Value::Raw(raw)),
        FieldKind::Word if raw.len() >= 2 => Value::Int(u16::from_le_bytes([raw[0], raw[1]]) as i64),
        FieldKind::LargeInt => int(8).map(Value::Int).unwrap_or(Value::Raw(raw)),
        FieldKind::Float if raw.len() >= 8 => Value::Float(f64::from_le_bytes(raw[..8].try_into().unwrap())),
        FieldKind::Blob => Value::Blob(raw),
        _ => Value::Raw(raw),
    }
}

impl fmt::Display for Value<'_> {
    /// Texto do valor (sem aspas). Null vira vazio: quem grava decide como representar NULL.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => Ok(()),
            Value::Text(s) => f.write_str(s),
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(x) => {
                if x.is_finite() {
                    write!(f, "{x}")
                } else {
                    Ok(())
                }
            }
            Value::Bool(b) => f.write_str(if *b { "true" } else { "false" }),
            Value::Date(d) => write!(f, "{}", d.format("%Y-%m-%d")),
            Value::Time(ms) => write!(f, "{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000),
            Value::Timestamp(t) => write!(f, "{}", t.format("%Y-%m-%d %H:%M:%S%.3f")),
            Value::Blob(b) | Value::Raw(b) => {
                for x in *b {
                    write!(f, "{x:02x}")?;
                }
                Ok(())
            }
            // Formato bytea do PostgreSQL.
            Value::Bytes(b) => {
                f.write_str("\\x")?;
                for x in b {
                    write!(f, "{x:02x}")?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use std::io::Write;

    struct Spec {
        name: &'static str,
        type_id: u8,
        decl: u16,
        size: u16,
    }

    /// Monta um .dat sintético com um registro, no mesmo layout das bases reais.
    fn build(specs: &[Spec], values: &[Option<Vec<u8>>]) -> Vec<u8> {
        let mut offsets = Vec::new();
        let mut pos = RECORD_HEADER_SIZE;
        for s in specs {
            offsets.push(pos);
            pos += 1 + s.size as usize;
        }
        let record_size = pos + 3; // sobra no fim, como nas bases reais
        let mut file = vec![0u8; HEADER_SIZE + specs.len() * FIELD_DESC_SIZE];
        file[0x29..0x2D].copy_from_slice(&1u32.to_le_bytes());
        file[0x2D..0x2F].copy_from_slice(&(record_size as u16).to_le_bytes());
        file[0x2F..0x31].copy_from_slice(&(specs.len() as u16).to_le_bytes());
        for (i, s) in specs.iter().enumerate() {
            let d = HEADER_SIZE + i * FIELD_DESC_SIZE;
            file[d + 2] = s.name.len() as u8;
            file[d + 3..d + 3 + s.name.len()].copy_from_slice(s.name.as_bytes());
            file[d + 0xA4] = s.type_id;
            file[d + 0xA6..d + 0xA8].copy_from_slice(&s.decl.to_le_bytes());
            file[d + 0xA9..d + 0xAB].copy_from_slice(&s.size.to_le_bytes());
            file[d + 0xAC..d + 0xAE].copy_from_slice(&(offsets[i] as u16).to_le_bytes());
        }
        let mut rec = vec![0u8; record_size];
        rec[5..9].copy_from_slice(&42u32.to_le_bytes());
        for (i, v) in values.iter().enumerate() {
            if let Some(bytes) = v {
                rec[offsets[i]] = 1;
                rec[offsets[i] + 1..offsets[i] + 1 + bytes.len()].copy_from_slice(bytes);
            }
        }
        let mut hashed = rec[RECORD_HEADER_SIZE..].to_vec();
        for (i, s) in specs.iter().enumerate() {
            if s.type_id == 3 {
                hashed[offsets[i] - RECORD_HEADER_SIZE..offsets[i] - RECORD_HEADER_SIZE + 1 + s.size as usize].fill(0);
            }
        }
        rec[9..25].copy_from_slice(&md5::compute(&hashed).0);
        file.extend_from_slice(&rec);
        file
    }

    fn open(bytes: &[u8]) -> Table {
        let dir = std::env::temp_dir().join(format!("dbisam-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Nome único: no Windows um arquivo ainda mapeado não pode ser sobrescrito.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = dir.join(format!("T{n}.dat"));
        std::fs::File::create(&path).unwrap().write_all(bytes).unwrap();
        Table::open(&path).unwrap()
    }

    #[test]
    fn decodifica_todos_os_tipos_vistos() {
        let ms_ts = (NaiveDate::from_ymd_opt(2026, 3, 19).unwrap().num_days_from_ce() as f64) * 86_400_000.0 + 32_861_105.0;
        let specs = [
            Spec { name: "ID", type_id: 6, decl: 0, size: 4 },
            Spec { name: "Nome", type_id: 1, decl: 10, size: 11 },
            Spec { name: "Data", type_id: 2, decl: 0, size: 4 },
            Spec { name: "Hora", type_id: 10, decl: 0, size: 4 },
            Spec { name: "DataHora", type_id: 11, decl: 0, size: 8 },
            Spec { name: "Ativo", type_id: 4, decl: 0, size: 2 },
            Spec { name: "Valor", type_id: 7, decl: 0, size: 8 },
            Spec { name: "Obs", type_id: 3, decl: 0, size: 8 },
            Spec { name: "Vazio", type_id: 6, decl: 0, size: 4 },
        ];
        let date = NaiveDate::from_ymd_opt(2026, 3, 19).unwrap().num_days_from_ce();
        let values = vec![
            Some(7i32.to_le_bytes().to_vec()),
            Some(b"Jo\xe3o\0".to_vec()),
            Some(date.to_le_bytes().to_vec()),
            Some(33_766_035i32.to_le_bytes().to_vec()),
            Some(ms_ts.to_le_bytes().to_vec()),
            Some(vec![1, 0]),
            Some(0.123456789f64.to_le_bytes().to_vec()),
            Some(vec![9, 9, 9, 9, 1, 0, 0, 0]),
            None,
        ];
        let t = open(&build(&specs, &values));
        assert_eq!(t.slots(), 1);
        let r = t.record(0);
        assert!(r.is_active());
        assert_eq!(r.record_id(), 42);
        assert!(r.hash_ok(), "hash deve ignorar o ponteiro do memo");
        let v: Vec<String> = t.fields.iter().map(|f| r.value(f).to_string()).collect();
        assert_eq!(v[0], "7");
        assert_eq!(v[1], "João");
        assert_eq!(v[2], "2026-03-19");
        assert_eq!(v[3], "09:22:46.035");
        assert_eq!(v[4], "2026-03-19 09:07:41.105"); // ms / 86.400.000 = ordinal do dia
        assert_eq!(v[5], "true");
        assert_eq!(v[6], "0.123456789");
        assert!(matches!(r.value(&t.fields[7]), Value::Blob(_)));
        assert_eq!(r.value(&t.fields[8]), Value::Null);
    }

    fn put_segment(blb: &mut Vec<u8>, at: usize, prev: u32, next: u32, used: u16, total: u32, data: &[u8]) {
        if blb.len() < at + 18 + data.len() {
            blb.resize(at + 18 + data.len(), 0);
        }
        blb[at..at + 4].copy_from_slice(&prev.to_le_bytes());
        blb[at + 4..at + 8].copy_from_slice(&next.to_le_bytes());
        blb[at + 8..at + 10].copy_from_slice(&used.to_le_bytes());
        blb[at + 14..at + 18].copy_from_slice(&total.to_le_bytes());
        blb[at + 18..at + 18 + data.len()].copy_from_slice(data);
    }

    fn table_with_memo(k: u8, pointer: u32, blb: Vec<u8>) -> Table {
        let specs = [Spec { name: "Obs", type_id: 3, decl: 0, size: 8 }];
        let mut ptr = pointer.to_le_bytes().to_vec();
        ptr.extend_from_slice(&pointer.to_le_bytes());
        let mut dat = build(&specs, &[Some(ptr)]);
        dat[0x34] = k;
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("dbisam-memo-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("M.dat"), &dat).unwrap();
        std::fs::write(dir.join("M.blb"), &blb).unwrap();
        let mut t = Table::open(&dir.join("M.dat")).unwrap();
        t.fields[0].subtype = 22; // memo de texto
        t
    }

    #[test]
    fn memo_continuo_em_blocos_de_512() {
        // ponteiro 3 com bloco de 512 (k = 2): índice teto(2/2) = 1 -> posição 512 + 512
        let text: Vec<u8> = (0..700).map(|i| b'a' + (i % 26) as u8).collect();
        let mut blb = vec![0u8; 512];
        put_segment(&mut blb, 1024, 0, 0, 700, 700, &text);
        let t = table_with_memo(2, 3, blb);
        assert_eq!(t.blob_block_size, 512);
        let v = t.record(0).value_resolved(&t.fields[0], false).unwrap();
        assert_eq!(v, Value::Text(String::from_utf8(text).unwrap()));
    }

    #[test]
    fn memo_encadeado_em_blocos_de_256() {
        // ponteiro 2 (k = 1): índice 1 -> 512 + 256; continua no ponteiro 4 -> 512 + 3 * 256
        let part1 = vec![b'x'; 238];
        let part2 = b"FIM \xc7\xc3O".to_vec();
        let mut blb = vec![0u8; 512];
        put_segment(&mut blb, 768, 0, 4, 238, (238 + part2.len()) as u32, &part1);
        put_segment(&mut blb, 512 + 3 * 256, 2, 0, part2.len() as u16, 0, &part2);
        let t = table_with_memo(1, 2, blb);
        match t.record(0).value_resolved(&t.fields[0], false).unwrap() {
            Value::Text(s) => {
                assert_eq!(s.len(), 238 + "FIM ÇÃO".len());
                assert!(s.ends_with("FIM ÇÃO"));
            }
            v => panic!("esperava texto, veio {v:?}"),
        }
    }

    #[test]
    fn memo_fora_do_arquivo_da_erro() {
        let t = table_with_memo(1, 99, vec![0u8; 512]);
        assert_eq!(t.record(0).value_resolved(&t.fields[0], false), Err(MemoError::OutOfRange));
    }

    #[test]
    fn registro_alterado_quebra_o_hash() {
        let specs = [Spec { name: "ID", type_id: 6, decl: 0, size: 4 }];
        let mut bytes = build(&specs, &[Some(1i32.to_le_bytes().to_vec())]);
        let t = open(&bytes);
        assert!(t.record(0).hash_ok());
        let last = bytes.len() - 5;
        bytes[last] ^= 0xFF;
        let t = open(&bytes);
        assert!(!t.record(0).hash_ok());
    }
}
