# Storage Scanner

Analisador de espaço em disco para Windows, escrito em Rust. Varre um volume inteiro em
segundos e mostra, do maior para o menor, o que ocupa o disco: árvore com barras de percentual,
treemap, painel de tipos de arquivo e ações sobre os itens (abrir, mover, compactar, excluir).

Funciona com HDs, SSDs, pendrives, cartões SD, pastas de rede (UNC) e celulares/câmeras via MTP.

## Destaques

- **Rápido.** Em NTFS com privilégio de administrador lê a MFT diretamente (parser próprio);
  nos demais casos usa uma varredura multithread com *work stealing*
  (`GetFileInformationByHandleEx`, sem uma chamada de sistema por arquivo).
- **Cinco métricas:** tamanho, alocado em disco, nº de arquivos, última modificação e taxa de
  compressão NTFS (`1 − alocado/tamanho`).
- **Árvore virtualizada** (milhões de linhas sem travar), com ordenação por coluna e colunas
  configuráveis.
- **Treemap *squarified*** calculado em segundo plano, colorido por tipo, idade ou compressão,
  com zoom, *breadcrumb* e seleção sincronizada com a árvore.
- **Ações seguras:** Lixeira por padrão (com desfazer), exclusão permanente só após confirmação,
  itens de sistema bloqueados, programas abertos sem herdar a elevação do app.
- **Atualização incremental:** depois de excluir/mover/copiar/compactar, a árvore é corrigida em
  memória, sem nova varredura.
- Textos externalizados (`crates/app/locales/pt-BR.yml`, `rust-i18n`); apenas pt-BR implementado.

## Uso

```
storage-scanner.exe                      # abre a interface
storage-scanner.exe "D:\Projetos"        # abre e já analisa a pasta
storage-scanner.exe --cli C:\ --top 20   # modo texto (diagnóstico)
storage-scanner.exe --cli C:\ --backend walk|mft|auto
storage-scanner.exe --cli --volumes      # lista unidades
storage-scanner.exe --cli --devices      # lista dispositivos MTP
```

Sem administrador, o app usa a varredura comum e mostra uma faixa com o botão
**Reiniciar como administrador**; elevado, volumes NTFS locais usam a MFT. Se a leitura da MFT
falhar (BitLocker bloqueado, volume desmontado…), cai automaticamente para a varredura comum.

### Atalhos

| Tecla | Ação |
|---|---|
| `Ctrl+O` | Escolher pasta e analisar |
| `F5` | Analisar de novo |
| `Esc` | Parar a análise / fechar o menu |
| `↑ ↓` / `← →` | Navegar / recolher / expandir na árvore |
| `Enter` | Expandir ou recolher a pasta selecionada |
| `Ctrl+Enter` | Abrir o item selecionado |
| `Delete` / `Shift+Delete` | Lixeira / exclusão permanente (com confirmação) |
| `Ctrl+C` | Copiar o caminho |
| `Ctrl+E` | Exportar a árvore para CSV |
| `Alt+↑` | Subir um nível no treemap |

Clique direito em um item da árvore ou do treemap abre o menu de contexto (inclui
**Mais opções do Windows…**, o menu nativo do shell).

### Configurações

`%APPDATA%\StorageScanner\config.toml` (tema claro/escuro/sistema, métrica padrão, colunas
visíveis, alvos recentes). Um arquivo inválido é preservado como `config.toml.bad`.
Registros e falhas fatais vão para `%APPDATA%\StorageScanner\app.log`.

O botão ⚙ também liga/desliga **Analisar com Storage Scanner** no menu de contexto do Explorer
(somente `HKCU`; nada é escrito fora do seu usuário).

## Compilar

Requisitos: Rust estável (`stable-x86_64-pc-windows-msvc`) e as *Build Tools* do Visual Studio.

```
cargo build --release -p storage-scanner
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Teste de integração MFT × Walker em um VHD NTFS (precisa de terminal de administrador):

```
cargo test -p ss-scan --test mft_vhd -- --ignored --nocapture
```

### Instalador (MSI)

Com o [WiX Toolset v3](https://github.com/wixtoolset/wix3/releases) e o `cargo-wix`:

```
cargo install cargo-wix
set WIX=C:\caminho\para\wix314-binaries\
cargo wix -p storage-scanner --nocapture
```

O MSI sai em `target\wix\`. Fonte do instalador: `crates/app/wix/main.wxs`.
**Atenção:** o `.wxs` foi escrito e o `cargo-wix` instalado, mas o MSI ainda não foi gerado nem
testado em uma instalação limpa (veja *Limitações conhecidas*).

## Arquitetura

Workspace Cargo com quatro crates:

| Crate | Conteúdo |
|---|---|
| `ss-core` | Árvore em arena (nós de ~56 bytes, nomes em arena única, filhos em CSR), agregação, ordenação, métricas, treemap, estatísticas por extensão, exportação CSV, formatação. Sem I/O. |
| `ss-scan` | Backends: MFT (`mft/`), varredura multithread (`walk/`), MTP/WPD (`mtp/`); volumes e elevação. |
| `ss-ops` | Ações via shell do Windows: `IFileOperation`, abrir/revelar no Explorer, zip, compressão NTFS, menu nativo, integração com o Explorer. |
| `storage-scanner` (`crates/app`) | Interface egui/eframe, CLI de diagnóstico, configuração e logs. |

Detalhes de projeto relevantes:

- **MFT:** validação de assinatura e *fixups*, run lists (inclusive MFT fragmentada com
  `$ATTRIBUTE_LIST`), `$STANDARD_INFORMATION`/`$FILE_NAME`/`$DATA`, registros de extensão,
  *hard links* contados uma vez, ADS e `WofCompressedData`, arquivos esparsos/comprimidos,
  metarquivos agrupados em *[Arquivos de sistema NTFS]* e órfãos/ciclos em *[Órfãos]*.
  A leitura do volume é somente leitura.
- **Walker:** cada diretório é uma tarefa; uma thread por núcleo em disco local, 3× em rede;
  threads com prioridade reduzida para manter a interface fluida; caminhos `\\?\` (longos).
- **MTP:** thread dedicada com COM, propriedades em lote (`IPortableDevicePropertiesBulk`) com
  *fallback* por objeto, detecção de travamento (90 s) e cancelamento. O MTP não expõe blocos,
  então alocado = tamanho. Ações disponíveis: copiar para o PC e excluir.

## Desempenho (medido nesta máquina, SSD NVMe, cache quente, sem administrador)

| Alvo | Arquivos | Tempo | Pico de memória do processo |
|---|---|---|---|
| `C:\Users` | 254 mil | 0,42 s | 68 MB |
| `C:\Windows` | 170 mil | 1,5 s | 71 MB |
| `C:\` (107 GB) | 547 mil | 1,2 s | 153 MB |

A árvore em si ocupa ~86 bytes por nó incluindo nomes (633 mil nós = 57 MB). O pico do processo
é maior por causa dos buffers de enumeração e da fase de montagem.

## Limitações conhecidas

- O caminho MFT foi validado com registros sintéticos (testes unitários) e deve ser conferido
  com o teste em VHD acima; ele não pôde ser executado em sessão não elevada.
- O backend MTP compila e segue a API do WPD, mas não foi exercitado com um aparelho real.
- Tamanhos de arquivos residentes na MFT (muito pequenos) aparecem como alocado 0, como na
  MFT; a varredura comum informa o tamanho de cluster.
- Hard links só são detectados pelo backend MFT; a varredura comum conta cada nome.
- O instalador MSI (`crates/app/wix/main.wxs`) não foi compilado nem testado em máquina/VM limpa.
- Interface somente em pt-BR (os textos já estão externalizados para novos idiomas).
