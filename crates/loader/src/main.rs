use memmap2::Mmap;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::env;

#[derive(Deserialize, Debug)]
struct Column {
    name: String,
    field_type: String,
    offset: u32,
}

#[derive(Deserialize, Debug)]
struct TableConfig {
    record_size: u32,
    columns: Vec<Column>,
}

type SchemaConfig = BTreeMap<String, TableConfig>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let target_table = env::var("TARGET_TABLE")
        .expect("ERRO: TARGET_TABLE não definida no .env");
    let base_path = env::var("DB_PATH")
        .expect("ERRO: DB_PATH não definida no .env");
    
    let toml_content = std::fs::read_to_string("schema.toml")
        .expect("ERRO: schema.toml não encontrado na raiz");
        
    let schema: SchemaConfig = toml::from_str(&toml_content)?;
    
    let config = schema.get(&target_table)
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

    while i + row_size <= mmap.len() {
        let row_data = &mmap[i..i + row_size];

        if row_data[0] == 0 {
            let mut row_values = Vec::new();

            for col in &config.columns {
                let start = col.offset as usize;
                
                let val = match col.field_type.as_str() {
                    "S" => {
                        decode_windows1252(&row_data[start..])
                    },
                    "I" => {
                        let bytes: [u8; 4] = row_data[start..start+4].try_into().unwrap_or([0; 4]);
                        i32::from_le_bytes(bytes).to_string()
                    },
                    "F" | "D" => {
                        let bytes: [u8; 8] = row_data[start..start+8].try_into().unwrap_or([0; 8]);
                        format!("{:.4}", f64::from_le_bytes(bytes))
                    },
                    _ => "".to_string(),
                };
                row_values.push(val);
            }
            wtr.write_record(&row_values)?;
            count += 1;
        }

        i += row_size;
        if count >= total_rows { break; }
    }

    wtr.flush()?;
    println!("Finalizado: {} registros extraídos para {}", count, output_name);
    Ok(())
}

fn decode_windows1252(bytes: &[u8]) -> String {
    let (res, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    res.split('\0').next().unwrap_or("").trim().to_string()
}