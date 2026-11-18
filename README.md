# 🛠️ DBISAM-Translate

Conjunto de ferramentas em Rust de alta performance para engenharia reversa e extração de dados de arquivos binários **DBISAM (.dat)**, sem driver.

Workspace com três crates:

* **dbisam** (biblioteca): lê a estrutura direto do cabeçalho do `.dat`, percorre os registros e decodifica cada tipo. É a base dos outros dois.
* **DBISAM-Scan**: varre uma pasta de `.dat` e gera o `schema.toml` com a estrutura de todas as tabelas.
* **DBISAM-Loader**: extrai tabelas para CSV pronto para o `COPY` do PostgreSQL, usando `mmap` e vários núcleos.

---

## 📐 Formato DBISAM 4 (conferido em bases reais)

* **Cabeçalho (512 bytes):** `0x29` total de registros (u32), `0x2D` tamanho do registro (u16), `0x2F` número de campos (u16).
  O total de registros pode ficar **atrás** do arquivo quando ele é copiado com o sistema gravando: o Loader lê todas as posições, não para no total do cabeçalho.
* **Descritor de campo (768 bytes cada, a partir de `0x200`):** `[2]` tamanho do nome, `[3..]` nome, `[0xA4]` tipo, `[0xA5]` subtipo, `[0xA6]` tamanho declarado (u16), `[0xA9]` bytes no registro (u16), `[0xAC]` posição no registro (u16).
* **Registro:** `[0]` situação (0 = ativo), `[5..9]` RecordID (sobe a cada inserção), `[9..25]` RecordHash = MD5 de `registro[25..]` com os campos memo zerados (muda quando o registro é alterado: serve para replicação incremental). Depois vêm os campos.
* **Campo:** 1 byte de nulo (0 = NULL, 1 = tem valor) e depois o valor.

| tipo | DBISAM | bytes | como é gravado |
|---|---|---|---|
| 1 | String | declarado + 1 | Windows-1252, termina em `\0` |
| 2 | Date | 4 | dias, ordinal (01/01/0001 = 1) |
| 3 | Blob/Memo | 8 | ponteiro para o `.blb` (subtipo 22 memo, 26 imagem) |
| 4 | Boolean | 2 | 0/1 |
| 6 | Integer | 4 | i32 (subtipo 29 = autoincremento) |
| 7 | Float | 8 | f64 (subtipo 21 = moeda) |
| 10 | Time | 4 | milissegundos desde a meia-noite |
| 11 | TimeStamp | 8 | f64 em milissegundos; ms / 86.400.000 = ordinal do dia |

Tipos 5 (SmallInt), 8 (BCD), 9 (Bytes), 12 (Word), 13 (LargeInt) e 14 (AutoInc) seguem a numeração do DBISAM mas **ainda não foram vistos em base real**: o Scan e o Loader avisam quando aparecem.

### Memos (arquivo `.blb`)

* Tamanho do bloco = 256 × `.dat[0x34]` (vistos 256 e 512; varia por tabela). Cabeçalho de 512 bytes (`0x1D` = total de blocos); o bloco de índice `i` (a partir de 0) começa em `512 + i × bloco`.
* O campo memo no registro guarda dois u32: o **número do bloco** inicial (a partir de 1: índice = ponteiro − 1, para qualquer tamanho de bloco) e o **id do memo**. O maior ponteiro de cada tabela é praticamente o total de blocos do arquivo.
* Cada trecho começa com 18 bytes: anterior (u32, 0 no primeiro), próximo (u32, número de bloco, 0 = fim), bytes de dados do trecho (u16, contínuos, podem atravessar blocos), id (u32, **igual ao 2º u32 do ponteiro**) e tamanho total do memo (u32, só no primeiro trecho). Texto em Windows-1252.
* O id do cabeçalho é sempre conferido. Se não bater (bloco reaproveitado, `.blb` copiado em momento diferente do `.dat`), o memo sai NULL e é contado como ilegível: nunca se devolve o texto de outro registro.
* O RecordHash do registro é calculado com os campos memo zerados: alterar só o texto de um memo pode não mudar o hash.

> [!WARNING]
> Versões anteriores deste projeto (até o commit `1a6eaf3`) dividiam o ponteiro pelo tamanho do bloco. Dava o texto certo só em tabelas de blocos de 256 bytes; nas de 512 devolvia o texto de **outro registro** (válido na estrutura, por isso passava nos testes de formato). Quem gravou memos com essas versões precisa regravar.

---

## 🔍 DBISAM-Scan

### Como gerar o executável
`cargo build -p DBISAM-Scan --release` (sai em `target/release/DBISAM-Scan.exe`).

### Como usar
Coloque o executável na pasta dos `.dat` (ou defina `DB_PATH` no `.env`) e execute. Ele gera o `schema.toml`:

```toml
[Vendedores]
record_size = 496

[[Vendedores.columns]]
name = "ID"
field_type = "I"
offset = 25
length = 4
type_id = 6
size = 4
```

`field_type`: `S` texto, `I` inteiro (`length` 1 = booleano, 2 = smallint, 4 = integer), `F` float, `D` data, `T` hora, `TS` data e hora, `M` memo/blob, `L` inteiro de 8 bytes, `X` bytes/BCD/desconhecido. `type_id` e `size` são o tipo e o tamanho originais do DBISAM. Quem lê o formato antigo (name, field_type, offset, length) continua funcionando e trata os códigos novos como texto.

---

## 🚀 DBISAM-Loader

O Loader **não precisa do `schema.toml`**: lê a estrutura de cada `.dat`, então continua certo quando o sistema altera uma tabela (coluna nova, campo maior).

### Como gerar o executável
`cargo build -p DBISAM-Loader --release` (sai em `target/release/DBISAM-Loader.exe`).

### Como usar
Variáveis no `.env` (ao lado do executável) ou no ambiente:

```env
DB_PATH=C:\caminho\para\os\arquivos_dat
TARGET_TABLE=NFMestre            # uma tabela, várias separadas por vírgula, ou * para todas
OUTPUT_DIR=C:\saida              # opcional (padrão: pasta atual)
RECORD_META=1                    # opcional: colunas _record_id e _record_hash
VERIFY_HASH=0                    # opcional: não confere o hash de cada registro
BLOB_BINARY=1                    # opcional: grava blobs binários (imagens) como bytea \x...; padrão NULL
```

Saída `extracao_<tabela>.csv`: cabeçalho com os nomes das colunas; **NULL = campo vazio sem aspas**; texto sempre entre aspas (texto vazio = `""`); datas `AAAA-MM-DD`; hora `HH:MM:SS.mmm`; data e hora `AAAA-MM-DD HH:MM:SS.mmm`; números com precisão total; booleanos `true`/`false`. Carrega direto com `COPY tabela FROM 'arquivo' (FORMAT csv, HEADER true)`.

Memos de texto são lidos do `.blb` que estiver ao lado do `.dat` (mesmo nome). No fim de cada tabela o Loader mostra registros, tempo, total do cabeçalho x posições no arquivo, apagados, registros com hash inválido (registro pela metade: arquivo em uso), memos lidos, memos sem o `.blb` e memos ilegíveis (`.blb` copiado em outro momento que o `.dat`). Memo sem `.blb` ou ilegível sai NULL.

> [!IMPORTANT]
> Copie o `.dat` e o `.blb` juntos (de preferência de uma cópia de sombra VSS): o `.blb` de outro momento pode apontar memos para blocos já reaproveitados.

### Desempenho medido
9 tabelas reais (3,3 milhões de registros, 5,7 GB de `.dat` + `.blb`), com conferência de hash: **5 s** no total, arquivos já em cache do sistema.

### Validação
Comparado registro a registro com um decodificador independente (todas as linhas de 9 tabelas e os 5,1 milhões de memos de 6 tabelas, achados só pelo id do cabeçalho do `.blb`, sem diferença) e com o BI que lê a mesma base por outro caminho (faturamento de um mês por loja igual ao centavo).

### Em uso
O crate `dbisam` é a base do Alembro Integrador (replicação incremental do BM para PostgreSQL usando RecordID + RecordHash como chave de mudança). Em produção, 119 tabelas (21,9 milhões de linhas) carregam em ~4 min e o ciclo incremental leva ~70 s lendo a pasta do BM em uso.

## Licença

Este projeto está licenciado sob Licença - veja o arquivo [LICENSE](./LICENSE) para detalhes.

Desenvolvido com ❤️ por Murilo de Souza
