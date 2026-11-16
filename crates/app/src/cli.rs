//! Modo CLI de diagnóstico: `storage-scanner --cli <caminho> [--top N] [--backend auto|walk|mft] | --volumes | --devices`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use rust_i18n::t;
use ss_core::format::{format_count, format_size};
use ss_core::sort::{sorted_children, top_nodes};
use ss_core::{Metric, Tree};
use ss_scan::{Backend, Labels, Progress, ScanTarget};

pub struct CliArgs {
    pub path: PathBuf,
    pub top: usize,
    pub backend: String,
    pub list_volumes: bool,
    pub list_devices: bool,
}

/// Interpreta os argumentos; `None` se não for o modo CLI.
pub fn parse(args: &[String]) -> Option<Result<CliArgs, String>> {
    let pos = args.iter().position(|a| a == "--cli")?;
    let mut path = None;
    let mut top = 20usize;
    let mut backend = "auto".to_string();
    let mut list_volumes = false;
    let mut list_devices = false;
    let mut it = args.iter().skip(pos + 1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--top" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => top = v,
                None => return Some(Err("--top requer um número".into())),
            },
            "--volumes" => list_volumes = true,
            "--devices" => list_devices = true,
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
        Some(path) => Ok(CliArgs {
            path,
            top,
            backend,
            list_volumes,
            list_devices,
        }),
        None if list_volumes || list_devices => Ok(CliArgs {
            path: PathBuf::new(),
            top,
            backend,
            list_volumes,
            list_devices,
        }),
        None => Err(
            "uso: storage-scanner --cli <caminho> [--top N] [--backend auto|walk|mft] | --volumes | --devices"
                .into(),
        ),
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
    if args.list_devices {
        return print_devices();
    }
    if args.list_volumes {
        print_volumes();
        return 0;
    }
    let Some(backend) = Backend::parse(&args.backend) else {
        eprintln!(
            "backend desconhecido: {} (use auto, walk ou mft)",
            args.backend
        );
        return 2;
    };
    let target = ScanTarget::Path(args.path.clone());
    let progress = std::sync::Arc::new(Progress::default());
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let labels = Labels {
        system_files: t!("tree.system_files").to_string(),
        orphans: t!("tree.orphans").to_string(),
    };

    let start = Instant::now();
    let report = match ss_scan::scan(&target, backend, &labels, &progress, &cancel) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("erro: {e}");
            return 1;
        }
    };
    let elapsed = start.elapsed();
    println!(
        "Backend:     {}",
        if report.backend == Backend::Mft {
            "MFT"
        } else {
            "Walker"
        }
    );
    if let Some(reason) = &report.fallback_reason {
        println!("Aviso:       MFT indisponível, usando o Walker ({reason})");
    }
    if report.backend == Backend::Walk
        && backend == Backend::Auto
        && report.fallback_reason.is_none()
        && !ss_scan::elevation::is_elevated()
        && is_local_ntfs(&args.path)
    {
        println!(
            "Dica:        execute como administrador para o modo rápido (MFT) em volumes NTFS"
        );
    }
    let tree = report.tree;
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
    println!(
        "Alocado:     {} ({} bytes)",
        format_size(root.allocated),
        format_count(root.allocated)
    );
    println!(
        "Compressão:  {}",
        ss_core::format::format_percent(ss_core::metrics::compression_ratio(root))
    );
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

fn print_volumes() {
    println!("Raiz   Sist.     Tipo            Total      Livre  Rótulo");
    for v in ss_scan::volumes::list_volumes() {
        println!(
            "{:<6} {:<8} {:<10} {:>10} {:>10}  {}",
            v.root,
            v.fs,
            format!("{:?}", v.kind),
            format_size(v.total),
            format_size(v.free),
            v.label
        );
    }
    println!(
        "
Elevado: {}",
        if ss_scan::elevation::is_elevated() {
            "sim"
        } else {
            "não"
        }
    );
}

fn print_devices() -> i32 {
    match ss_scan::mtp::list_devices() {
        Ok(devs) if devs.is_empty() => println!("Nenhum dispositivo portátil (MTP) conectado."),
        Ok(devs) => {
            for d in devs {
                println!("{}\t{}", d.name, d.id);
            }
        }
        Err(e) => {
            eprintln!("erro: {e}");
            return 1;
        }
    }
    0
}

/// A dica de elevação só faz sentido para volumes NTFS locais.
fn is_local_ntfs(path: &std::path::Path) -> bool {
    use ss_scan::volumes::{DriveKind, volume_info, volume_root_of};
    volume_root_of(path)
        .map(|r| volume_info(&r))
        .is_some_and(|v| v.is_ntfs() && v.kind != DriveKind::Remote)
}
