# CLAUDE.md — DBISAM-Translate

Contexto para retomar o trabalho. **Repositório público:** não citar clientes, caminhos internos, senhas nem dados reais aqui ou nos commits. Formato DBISAM e uso das ferramentas: `README.md`.

## O que é

Leitura de tabelas DBISAM 4 (`.dat` + `.blb`) direto do arquivo, sem driver. Workspace Rust:

- `crates/dbisam` (biblioteca): estrutura lida do cabeçalho do próprio `.dat`, registros, tipos, NULL, RecordID/RecordHash e memos do `.blb`. É a base de tudo; outros projetos (integrador DBISAM → PostgreSQL) dependem dela pelo git.
- `crates/scan` (DBISAM-Scan): gera `schema.toml`. Mantém o formato antigo (`name`, `field_type`, `offset`, `length`), que o NewReport lê, e acrescenta `type_id` e `size`. Códigos novos de `field_type`: `T` hora, `TS` data e hora, `M` memo/blob, `L` inteiro de 8 bytes, `X` bytes/BCD/desconhecido.
- `crates/loader` (DBISAM-Loader): `.dat` → CSV para `COPY` do PostgreSQL (NULL = vazio sem aspas, texto sempre entre aspas). Não usa o `schema.toml`.

## Como o dono trabalha

- Responder em PT-BR, direto. Código em inglês; comentários e mensagens em PT-BR.
- Discutir antes de implementar quando ele pergunta "o que acha / como funciona".
- Dizer com honestidade o que foi verificado e o que não foi.

## Estado (nov/2026, versão 0.2)

Refeito em nov/2026. Validado em 9 tabelas reais (3,3 milhões de registros; memos de 6 tabelas, 5,1 milhões, conferidos pelo id do cabeçalho): igual a um decodificador independente escrito à parte, em todas as linhas e memos, e o faturamento de um mês bateu ao centavo com um BI que lê a mesma base por outro caminho. 9 tabelas (5,7 GB) em ~5 s com os arquivos em cache.

Em produção (18/11/2026): usado pelo Alembro Integrador (outro repositório), que replica 119 tabelas do BM para o PostgreSQL: carga inicial de 21,9 milhões de linhas em 4 min; ciclo incremental de ~72 s a cada 15 min, lendo a pasta de bases do BM com ele aberto, sem erro de leitura nem de hash. Os dados reais conferiram com o BI e as ligações entre tabelas bateram (99 a 100%), inclusive os memos (corrigidos no commit `555999f`).

## Armadilhas já encontradas (não repetir)

- **Nunca confiar num `schema.toml` antigo para ler dados:** a estrutura muda com as versões do sistema (coluna nova, campo 1 byte maior) e a leitura sai desalinhada sem erro. O loader antigo fazia isso. Sempre ler os descritores do `.dat`.
- **O total de registros do cabeçalho (`0x29`) fica atrás** quando o arquivo é copiado em uso: percorrer todas as posições do arquivo.
- **Tamanho do campo = `descritor[0xA9]`** (não deduzir pela distância entre campos). Booleano tem 2 bytes; data e hora tem 8 (double em ms, ordinal); hora 4 (ms desde a meia-noite).
- **Byte de nulo** antes de cada campo (0 = NULL).
- **RecordHash = MD5 de `registro[25..]` com os campos memo zerados.** Mudar só o texto de um memo pode não mudar o hash.
- **`.blb`:** bloco = 256 × `.dat[0x34]` (256 ou 512, varia por tabela). O ponteiro é o **número do bloco** (índice = p1 − 1, qualquer que seja o tamanho do bloco) e o 2º u32 é o **id do memo**, igual ao id do cabeçalho do trecho: sempre conferir. Trechos contínuos podem atravessar blocos (`bytes do trecho` > bloco − 18).
- **ERRO JÁ COMETIDO (18/11/2026):** a 1ª versão dividia o ponteiro pelo tamanho do bloco. Funcionava só na tabela de blocos de 256 bytes; nas de 512 lia o texto de OUTRO registro, e o teste "estrutura válida + cadeia completa" passava. Memo só se prova pelo id do cabeçalho ou pelo conteúdo (ex.: `ExProps` é sempre `CHAVE=valor`), nunca só pela estrutura.
- Texto em Windows-1252 decodificado como WHATWG (`encoding_rs`): bytes sem caractere (0x81, 0x8D...) viram o mesmo código; o Python precisa de um tratador próprio para comparar.
- No Windows, arquivo mapeado não pode ser sobrescrito: nos testes, usar um nome de arquivo por teste.
- `.dat` e `.blb` copiados em momentos diferentes podem apontar memos para blocos reaproveitados.

## Pendências

- Registros apagados: nenhum nas amostras; supõe-se situação ≠ 0 = apagado (não confirmado).
- Tipos ainda não vistos em base real: 5 SmallInt, 8 BCD, 9 Bytes, 12 Word, 13 LargeInt, 14 AutoInc (Scan e Loader avisam).
- Imagens (blob subtipo 26): leitura implementada (`BLOB_BINARY=1`), mas não testada com arquivo real.
- Leitura com o sistema gravando ao mesmo tempo: não testada ao vivo (só cópias). O leitor não pega locks do DBISAM: não bloqueia o sistema, mas pode ver registro pela metade (o hash detecta) ou tabelas em instantes diferentes. Para consistência, ler de uma cópia de sombra (VSS).
- O NewReport (outro projeto) ainda trata `T`, `TS` e `M` como texto.

## Comandos

```bash
cargo test --workspace
cargo build --release
DB_PATH=<pasta dos .dat> TARGET_TABLE='*' OUTPUT_DIR=<saida> target/release/DBISAM-Loader.exe
```
