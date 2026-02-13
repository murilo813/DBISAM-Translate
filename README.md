# 🛠️ DBISAM-Translate

Conjunto de ferramentas em Rust de alta performance para engenharia reversa e extração de dados de arquivos binários **DBISAM (.dat)**. 

Este projeto é dividido em dois módulos principais integrados via um Workspace Rust: um para mapeamento estrutural e outro para extração massiva.

## 📁 Estrutura do Projeto

* **DBISAM-Scan**: Varre diretórios em busca de arquivos `.dat`, identifica a estrutura de colunas, tipos de dados e offsets, gerando um mapa técnico em TOML.
* **DBISAM-Loader**: Utiliza o mapa gerado pelo Scan para realizar a extração dos dados binários para CSV, utilizando `Memory Mapping (mmap)` para performance extrema.

---

## 🔍 DBISAM-Scan

O Scan é a ferramenta de reconhecimento. Ele identifica tipos como String, Integer, Float e Date, além de calcular o `record_size` real (incluindo o byte de status/delete do DBISAM).

### Baixar o executável
1. Vá até a aba [Releases](https://github.com/murilo813/DBISAM-Translate/releases).
2. Baixe o `scan.exe`.

Ou se preferir

### Como gerar o executável
1. Na pasta raiz do DBISAM-Translate rode 'cargo build -p DBISAM-Scan --release'.
2. O executável será gerado na pasta target/release (DBISAM-Scan.exe).

### Como usar
1. Jogue o executável dentro da pasta que contém os arquivos `.dat`.
2. Execute o `DBISAM-Scan.exe`.
3. Um arquivo `schema.toml` será gerado com todo o mapeamento das tabelas.

### 📄 Exemplo de `schema.toml` gerado:
O Scan identifica a estrutura e organiza as colunas por offset:

```toml
[Tabela1]
record_size = 2864

[[Tabela1.columns]]
name = "ID"
field_type = "I"
offset = 25

[Tabela2]
record_size = 152

[[Tabela2.columns]]
name = "nome"
field_type = "S"
offset = 40
```

---

## 🚀 DBISAM-Loader

O Loader é o motor de extração. Ele consome o `schema.toml` e despeja os dados em arquivos CSV prontos para importação em bancos modernos (PostgreSQL, MySQL, etc).

### Baixar o executável
1. Vá até a aba [Releases](https://github.com/murilo813/DBISAM-Translate/releases).
2. Baixe o `DBISAM-Loader.exe`.

Ou se preferir

### Como gerar o executável
1. Na pasta raiz do DBISAM-Translate rode 'cargo build -p DBISAM-Loader --release'.
2. O executável será gerado na pasta target/release (DBISAM-Loader.exe).

### Como usar
O schema.toml será usado aqui, o coloque na pasta raíz do seu DBISAM-Loader.exe, ele exige também um arquivo `.env` na raiz do projeto para funcionar:
```env
TARGET_TABLE=NomeDaTabela (sem o .dat)
DB_PATH=C:\caminho\para\os\arquivos_dat
```
---

## Licença

Este projeto está licenciado sob Licença - veja o arquivo [LICENSE](./LICENSE) para detalhes.

Desenvolvido com ❤️ por Murilo de Souza
