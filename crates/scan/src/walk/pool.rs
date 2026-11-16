//! Pool work-stealing: cada diretório é uma tarefa.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use crossbeam_deque::{Injector, Stealer, Worker};
use ss_core::{NodeId, NodeInfo};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};

use super::dir_enum::{Entry, read_dir};
use crate::Progress;

pub struct Task {
    pub id: NodeId,
    pub path: PathBuf,
}

/// Nó produzido por um worker, ainda sem posição na árvore final.
pub struct LocalNode {
    pub id: NodeId,
    pub parent: NodeId,
    pub name: String,
    pub info: NodeInfo,
}

#[derive(Default)]
pub struct WorkerOutput {
    pub nodes: Vec<LocalNode>,
    /// Diretórios cuja leitura falhou.
    pub failed_dirs: Vec<NodeId>,
}

struct Shared<'a> {
    injector: Injector<Task>,
    stealers: Vec<Stealer<Task>>,
    /// Tarefas enfileiradas + em execução; zero = terminou.
    pending: AtomicUsize,
    next_id: AtomicU32,
    progress: &'a Progress,
    cancel: &'a AtomicBool,
}

/// Percorre `root` (já com id 0 reservado) e devolve os nós dos filhos e o total de ids.
pub fn run(
    root: PathBuf,
    threads: usize,
    progress: &Progress,
    cancel: &AtomicBool,
) -> (Vec<WorkerOutput>, u32) {
    let threads = threads.max(1);
    let workers: Vec<Worker<Task>> = (0..threads).map(|_| Worker::new_lifo()).collect();
    let shared = Shared {
        injector: Injector::new(),
        stealers: workers.iter().map(Worker::stealer).collect(),
        pending: AtomicUsize::new(1),
        next_id: AtomicU32::new(1),
        progress,
        cancel,
    };
    shared.injector.push(Task { id: 0, path: root });

    let outputs = std::thread::scope(|s| {
        let handles: Vec<_> = workers
            .into_iter()
            .map(|w| {
                let shared = &shared;
                s.spawn(move || worker_loop(shared, w))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let total = shared.next_id.load(Ordering::Relaxed);
    (outputs, total)
}

fn find_task(local: &Worker<Task>, shared: &Shared) -> Option<Task> {
    local.pop().or_else(|| {
        std::iter::repeat_with(|| {
            shared
                .injector
                .steal_batch_and_pop(local)
                .or_else(|| shared.stealers.iter().map(|s| s.steal()).collect())
        })
        .find(|s| !s.is_retry())
        .and_then(|s| s.success())
    })
}

fn worker_loop(shared: &Shared, local: Worker<Task>) -> WorkerOutput {
    // Prioridade baixa: a interface continua fluida enquanto a varredura usa todos os núcleos.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let mut out = WorkerOutput::default();
    let mut entries: Vec<Entry> = Vec::new();
    let mut buf: Vec<u64> = Vec::new();
    loop {
        let Some(task) = find_task(&local, shared) else {
            if shared.pending.load(Ordering::Acquire) == 0 || shared.cancel.load(Ordering::Relaxed)
            {
                break;
            }
            std::thread::sleep(Duration::from_micros(200));
            continue;
        };
        if !shared.cancel.load(Ordering::Relaxed) {
            process(&task, shared, &local, &mut out, &mut entries, &mut buf);
        }
        shared.pending.fetch_sub(1, Ordering::AcqRel);
    }
    out
}

fn process(
    task: &Task,
    shared: &Shared,
    local: &Worker<Task>,
    out: &mut WorkerOutput,
    entries: &mut Vec<Entry>,
    buf: &mut Vec<u64>,
) {
    entries.clear();
    if let Err(e) = read_dir(&task.path, entries, buf) {
        tracing::debug!("falha ao ler {}: {e}", task.path.display());
        out.failed_dirs.push(task.id);
        shared.progress.errors.fetch_add(1, Ordering::Relaxed);
        // Entradas parciais, se houver, ainda são aproveitadas.
    }
    shared.progress.dirs.fetch_add(1, Ordering::Relaxed);
    if entries.is_empty() {
        return;
    }
    let first = shared
        .next_id
        .fetch_add(entries.len() as u32, Ordering::Relaxed);
    let (mut files, mut bytes) = (0u64, 0u64);
    for (i, e) in entries.drain(..).enumerate() {
        let id = first + i as u32;
        if e.is_traversable() {
            shared.pending.fetch_add(1, Ordering::AcqRel);
            local.push(Task {
                id,
                path: task.path.join(&e.name),
            });
        } else if !e.is_dir() {
            files += 1;
            bytes += e.size;
        }
        out.nodes.push(LocalNode {
            id,
            parent: task.id,
            info: NodeInfo {
                flags: e.flags,
                size: e.size,
                allocated: e.allocated,
                mtime: e.mtime,
            },
            name: e.name,
        });
    }
    shared.progress.files.fetch_add(files, Ordering::Relaxed);
    shared.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
}
