use memmap2::Mmap;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::Read;

#[derive(Deserialize, Debug)]
struct Column {
    name: String,
    field_type: String,
    offset: u32,
    length: u32,
}

#[derive(Deserialize, Debug)]
struct TableConfig {
    record_size: u32,
    columns: Vec<Column>,
}

type SchemaConfig = BTreeMap<String, TableConfig>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            let env_path = dir.join(".env");
            dotenvy::from_path(&env_path).ok();
        }
    }

    dotenvy::dotenv().ok();

    let target_table = env::var("TARGET_TABLE")
        .map(|v| v.replace('"', ""))
        .unwrap_or_else(|_| {
            println!("⚠️ TARGET_TABLE não encontrada no .env, usando padrão: Pessoas");
            "Pessoas".to_string()
        });

    let base_path = env::var("DB_PATH")
        .map(|v| v.replace('"', ""))
        .unwrap_or_else(|_| {
            let path = r"C:\BmSoft\Bases\zecao".to_string();
            println!("⚠️ DB_PATH não encontrado no .env, usando padrão: {}", path);
            path
        });

    let toml_content =
        std::fs::read_to_string("schema.toml").expect("ERRO: schema.toml não encontrado na raiz");

    let schema: SchemaConfig = toml::from_str(&toml_content)?;

    let config = schema
        .get(&target_table)
        .expect("ERRO: Tabela não encontrada no schema.toml");

    let file_path = format!(r"{}\{}.dat", base_path, target_table);
    let mut file = File::open(&file_path)?;

    let mut h = [0u8; 512];
    file.read_exact(&mut h)?;

    let total_rows = u32::from_le_bytes(h[0x29..0x2D].try_into()?);
    let total_fields = u16::from_le_bytes(h[0x2F..0x31].try_into()?);

    let row_size = config.record_size as usize;
    let data_offset = 0x200 + (total_fields as usize * 768);

    println!("Extraindo {} | {} registros...", target_table, total_rows);

    let mmap = unsafe { Mmap::map(&file)? };
    let output_name = format!("extracao_{}.csv", target_table.to_lowercase());
    let mut wtr = csv::Writer::from_path(&output_name)?;

    let headers: Vec<String> = config.columns.iter().map(|c| c.name.clone()).collect();
    wtr.write_record(&headers)?;

    let mut count = 0;
    let mut i = data_offset;
    let cols_len = config.columns.len();

    while i + row_size <= mmap.len() {
        if let Some(row_data) = mmap.get(i..i + row_size) {
            if row_data[0] == 0 {
                let mut row_values = Vec::with_capacity(cols_len);

                for col in &config.columns {
                    let start = col.offset as usize + 1;
                    let end = (start + col.length as usize).min(row_size);

                    let val = match col.field_type.as_str() {
                        "S" => {
                            if let Some(slice) = row_data.get(start..end) {
                                decode_windows1252(slice)
                            } else {
                                String::new()
                            }
                        }
                        "I" => {
                            if let Some(slice) = row_data.get(start..end) {
                                if col.length == 1 {
                                    if slice[0] == 0 {
                                        "false".to_string()
                                    } else {
                                        "true".to_string()
                                    }
                                } else {
                                    let mut b = [0u8; 4];
                                    let n = slice.len().min(4);
                                    b[..n].copy_from_slice(&slice[..n]);
                                    i32::from_le_bytes(b).to_string()
                                }
                            } else {
                                "0".to_string()
                            }
                        }
                        "F" => {
                            if let Some(slice) = row_data.get(start..end) {
                                let mut b = [0u8; 8];
                                let n = slice.len().min(8);
                                b[..n].copy_from_slice(&slice[..n]);
                                format!("{:.4}", f64::from_le_bytes(b))
                            } else {
                                "0.0000".to_string()
                            }
                        }
                        "D" => {
                            if let Some(slice) = row_data.get(start..end) {
                                let mut b = [0u8; 4];
                                let n = slice.len().min(4);
                                b[..n].copy_from_slice(&slice[..n]);
                                let days = i32::from_le_bytes(b);
                                if days > 0 {
                                    convert_dbisam_to_iso(days)
                                } else {
                                    String::new()
                                }
                            } else {
                                String::new()
                            }
                        }
                        "B" => "[BIN]".to_string(),
                        _ => String::new(),
                    };
                    row_values.push(val);
                }
                wtr.write_record(&row_values)?;
                count += 1;
            }
        }

        i += row_size;
        if count >= total_rows {
            break;
        }
    }

    wtr.flush()?;
    println!(
        "Finalizado: {} registros extraídos para {}",
        count, output_name
    );
    Ok(())
}

fn decode_windows1252(bytes: &[u8]) -> String {
    let (res, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    res.trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string()
}

fn convert_dbisam_to_iso(days: i32) -> String {
    let epoch_days = days - 719163;
    let seconds = (epoch_days as i64) * 86400;
    if let Some(t) = std::time::SystemTime::UNIX_EPOCH
        .checked_add(std::time::Duration::from_secs(seconds.max(0) as u64))
    {
        let datetime: chrono::DateTime<chrono::Utc> = t.into();
        return datetime.format("%Y-%m-%d").to_string();
    }
    "".to_string()
}
