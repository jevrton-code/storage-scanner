//! Modo CLI de diagnóstico: `storage-scanner --cli <caminho> [--top N] [--backend walk|mft]`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use ss_core::format::{format_count, format_size};
use ss_core::sort::{sorted_children, top_nodes};
use ss_core::{Metric, Tree};
use ss_scan::{Progress, ScanTarget, Scanner};

pub struct CliArgs {
    pub path: PathBuf,
    pub top: usize,
    pub backend: String,
}

/// Interpreta os argumentos; `None` se não for o modo CLI.
pub fn parse(args: &[String]) -> Option<Result<CliArgs, String>> {
    let pos = args.iter().position(|a| a == "--cli")?;
    let mut path = None;
    let mut top = 20usize;
    let mut backend = "auto".to_string();
    let mut it = args.iter().skip(pos + 1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--top" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => top = v,
                None => return Some(Err("--top requer um número".into())),
            },
            "--backend" => match it.next() {
                Some(v) => backend = v.clone(),
                None => return Some(Err("--backend requer um valor".into())),
            },
            other if other.starts_with("--") => {
                return Some(Err(format!("opção desconhecida: {other}")));
            }
            other => path = Some(PathBuf::from(other)),
        }
    }
    Some(match path {
        Some(path) => Ok(CliArgs { path, top, backend }),
        None => Err("uso: storage-scanner --cli <caminho> [--top N] [--backend walk|mft]".into()),
    })
}

/// Em build "windows" (sem console), liga a saída ao terminal que nos iniciou.
fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

pub fn run(args: CliArgs) -> i32 {
    attach_console();
    let target = ScanTarget::Path(args.path.clone());
    let progress = Progress::default();
    let cancel = AtomicBool::new(false);

    let start = Instant::now();
    let result = match args.backend.as_str() {
        "auto" | "walk" => ss_scan::walk::WalkScanner::local().scan(&target, &progress, &cancel),
        other => {
            eprintln!("backend desconhecido ou indisponível: {other}");
            return 2;
        }
    };
    let elapsed = start.elapsed();
    let tree = match result {
        Ok(t) => t,
        Err(e) => {
            eprintln!("erro: {e}");
            return 1;
        }
    };
    print_report(&tree, args.top, elapsed, progress.snapshot().3);
    0
}

fn print_report(tree: &Tree, top: usize, elapsed: std::time::Duration, errors: u64) {
    let root = tree.node(Tree::ROOT);
    println!("Alvo:        {}", tree.name(Tree::ROOT));
    println!("Tempo:       {:.2} s", elapsed.as_secs_f64());
    println!(
        "Tamanho:     {} ({} bytes)",
        format_size(root.size),
        format_count(root.size)
    );
    println!("Alocado:     {}", format_size(root.allocated));
    println!("Arquivos:    {}", format_count(root.files.into()));
    println!("Pastas:      {}", format_count(root.dirs.into()));
    println!("Sem acesso:  {}", format_count(errors));
    println!(
        "Memória:     {} ({} nós)",
        format_size(tree.heap_bytes() as u64),
        format_count(tree.len() as u64)
    );

    println!("\nMaiores itens em {}:", tree.name(Tree::ROOT));
    for id in sorted_children(tree, Tree::ROOT, Metric::Size)
        .into_iter()
        .take(top)
    {
        let n = tree.node(id);
        let pct = if root.size > 0 {
            n.size as f64 * 100.0 / root.size as f64
        } else {
            0.0
        };
        println!(
            "  {:>10}  {:>5.1}%  {}{}",
            format_size(n.size),
            pct,
            tree.name(id),
            if n.is_dir() { "\\" } else { "" }
        );
    }

    println!("\nMaiores arquivos:");
    for id in top_nodes(tree, Metric::Size, top, true) {
        println!(
            "  {:>10}  {}",
            format_size(tree.node(id).size),
            tree.path(id)
        );
    }
}
