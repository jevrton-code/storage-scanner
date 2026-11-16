//! Árvore em arena compacta (nós + nomes UTF-8 concatenados + filhos em CSR).

pub type NodeId = u32;

/// Marca "sem pai" / "sem nó".
pub const NONE: NodeId = u32::MAX;

pub mod flags {
    pub const DIR: u16 = 1 << 0;
    pub const REPARSE: u16 = 1 << 1;
    pub const COMPRESSED: u16 = 1 << 2;
    pub const SPARSE: u16 = 1 << 3;
    pub const HARDLINK: u16 = 1 << 4;
    pub const HIDDEN: u16 = 1 << 5;
    pub const SYSTEM: u16 = 1 << 6;
    pub const ACCESS_ERROR: u16 = 1 << 7;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Node {
    pub parent: NodeId,
    pub name_off: u32,
    pub name_len: u16,
    pub flags: u16,
    /// Tamanho lógico (agregado em diretórios).
    pub size: u64,
    /// Tamanho alocado em disco (agregado em diretórios).
    pub allocated: u64,
    /// Nº de arquivos (1 para um arquivo; soma recursiva para diretórios).
    pub files: u32,
    /// Nº de subpastas (recursivo; não conta o próprio nó).
    pub dirs: u32,
    /// FILETIME (100 ns desde 1601); máximo recursivo em diretórios.
    pub mtime: i64,
    pub child_start: u32,
    pub child_count: u32,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.flags & flags::DIR != 0
    }
    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }
}

/// Dados de um nó fornecidos por um backend de varredura.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeInfo {
    pub flags: u16,
    pub size: u64,
    pub allocated: u64,
    pub mtime: i64,
}

#[derive(Debug, Default)]
pub struct Tree {
    nodes: Vec<Node>,
    names: Vec<u8>,
    children: Vec<NodeId>,
}

impl Tree {
    pub const ROOT: NodeId = 0;

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn name(&self, id: NodeId) -> &str {
        let n = &self.nodes[id as usize];
        let s = &self.names[n.name_off as usize..n.name_off as usize + n.name_len as usize];
        // Os nomes só entram via `&str`, portanto são UTF-8 válido.
        std::str::from_utf8(s).unwrap_or("?")
    }
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        let n = &self.nodes[id as usize];
        &self.children[n.child_start as usize..(n.child_start + n.child_count) as usize]
    }
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        let p = self.nodes[id as usize].parent;
        (p != NONE).then_some(p)
    }

    /// Caminho completo, reconstruído subindo pelos pais.
    pub fn path(&self, id: NodeId) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            parts.push(self.name(c));
            cur = self.parent(c);
        }
        let mut out = String::new();
        for (i, p) in parts.iter().rev().enumerate() {
            if i > 0 && !out.ends_with('\\') {
                out.push('\\');
            }
            out.push_str(p);
        }
        out
    }

    /// Bytes ocupados pelas estruturas internas (para medir memória).
    pub fn heap_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.names.capacity()
            + self.children.capacity() * 4
    }

    /// `ancestor` é `id` ou um de seus ancestrais?
    pub fn is_ancestor_or_self(&self, ancestor: NodeId, mut id: NodeId) -> bool {
        loop {
            if id == ancestor {
                return true;
            }
            match self.parent(id) {
                Some(p) => id = p,
                None => return false,
            }
        }
    }
}

/// Constrói uma [`Tree`] a partir de nós com ids densos (0 = raiz) postos em qualquer ordem.
#[derive(Default)]
pub struct TreeBuilder {
    nodes: Vec<Node>,
    names: Vec<u8>,
}

impl TreeBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Garante que existam `n` slots (ids `0..n`).
    pub fn resize(&mut self, n: usize) {
        let blank = Node {
            parent: NONE,
            ..Node::default()
        };
        self.nodes.resize(n, blank);
    }

    /// Cria um nó novo no fim e devolve seu id.
    pub fn push(&mut self, parent: NodeId, name: &str, info: NodeInfo) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.resize(id as usize + 1);
        self.put(id, parent, name, info);
        id
    }

    /// Preenche o slot `id`.
    pub fn put(&mut self, id: NodeId, parent: NodeId, name: &str, info: NodeInfo) {
        // Corta em fronteira de caractere se passar de u16::MAX bytes.
        let mut len = name.len().min(u16::MAX as usize);
        while !name.is_char_boundary(len) {
            len -= 1;
        }
        let off = self.names.len() as u32;
        self.names.extend_from_slice(&name.as_bytes()[..len]);
        let is_dir = info.flags & flags::DIR != 0;
        self.nodes[id as usize] = Node {
            parent,
            name_off: off,
            name_len: len as u16,
            flags: info.flags,
            size: info.size,
            allocated: info.allocated,
            files: u32::from(!is_dir),
            dirs: 0,
            mtime: info.mtime,
            child_start: 0,
            child_count: 0,
        };
    }

    /// Liga flags adicionais num nó já preenchido.
    pub fn or_flags(&mut self, id: NodeId, f: u16) {
        self.nodes[id as usize].flags |= f;
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Monta o CSR de filhos e agrega tamanhos/contagens/datas de baixo para cima.
    pub fn finish(mut self) -> Tree {
        let n = self.nodes.len();
        for node in &mut self.nodes {
            node.child_count = 0;
        }
        for i in 0..n {
            let p = self.nodes[i].parent;
            if p != NONE && p as usize != i {
                self.nodes[p as usize].child_count += 1;
            }
        }
        let mut acc = 0u32;
        for node in &mut self.nodes {
            node.child_start = acc;
            acc += node.child_count;
        }
        let mut children = vec![NONE; acc as usize];
        let mut cursor: Vec<u32> = self.nodes.iter().map(|n| n.child_start).collect();
        for i in 0..n {
            let p = self.nodes[i].parent;
            if p != NONE && p as usize != i {
                children[cursor[p as usize] as usize] = i as NodeId;
                cursor[p as usize] += 1;
            }
        }
        // Ordem BFS a partir da raiz; agregação em ordem inversa.
        if n > 0 {
            let mut order: Vec<NodeId> = Vec::with_capacity(n);
            order.push(0);
            let mut head = 0;
            while head < order.len() {
                let id = order[head] as usize;
                head += 1;
                let (s, c) = (self.nodes[id].child_start, self.nodes[id].child_count);
                order.extend_from_slice(&children[s as usize..(s + c) as usize]);
            }
            for &id in order.iter().rev() {
                let id = id as usize;
                if !self.nodes[id].is_dir() {
                    continue;
                }
                let (s, c) = (self.nodes[id].child_start, self.nodes[id].child_count);
                let (mut size, mut alloc, mut files, mut dirs) = (0u64, 0u64, 0u32, 0u32);
                let mut mtime = self.nodes[id].mtime;
                for &ch in &children[s as usize..(s + c) as usize] {
                    let ch = &self.nodes[ch as usize];
                    size += ch.size;
                    alloc += ch.allocated;
                    files += ch.files;
                    dirs += ch.dirs + u32::from(ch.is_dir());
                    mtime = mtime.max(ch.mtime);
                }
                let node = &mut self.nodes[id];
                node.size = size;
                node.allocated = alloc;
                node.files = files;
                node.dirs = dirs;
                node.mtime = mtime;
            }
        }
        self.nodes.shrink_to_fit();
        self.names.shrink_to_fit();
        Tree {
            nodes: self.nodes,
            names: self.names,
            children,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size: u64, alloc: u64, mtime: i64) -> NodeInfo {
        NodeInfo {
            flags: 0,
            size,
            allocated: alloc,
            mtime,
        }
    }
    fn dir() -> NodeInfo {
        NodeInfo {
            flags: flags::DIR,
            ..Default::default()
        }
    }

    fn sample() -> Tree {
        let mut b = TreeBuilder::new();
        let root = b.push(NONE, "C:\\", dir());
        let a = b.push(root, "a", dir());
        let c = b.push(a, "c", dir());
        b.push(root, "x.txt", file(100, 4096, 5));
        b.push(a, "y.bin", file(1000, 4096, 9));
        b.push(c, "z.dat", file(10, 4096, 7));
        b.push(c, "w.dat", file(20, 4096, 1));
        b.finish()
    }

    #[test]
    fn aggregates_bottom_up() {
        let t = sample();
        let r = t.node(Tree::ROOT);
        assert_eq!(r.size, 1130);
        assert_eq!(r.allocated, 4 * 4096);
        assert_eq!(r.files, 4);
        assert_eq!(r.dirs, 2);
        assert_eq!(r.mtime, 9);
        let a = t.node(1);
        assert_eq!((a.size, a.files, a.dirs), (1030, 3, 1));
    }

    #[test]
    fn paths_and_children() {
        let t = sample();
        assert_eq!(t.children(Tree::ROOT).len(), 2);
        assert_eq!(t.path(2), "C:\\a\\c");
        assert_eq!(t.path(5), "C:\\a\\c\\z.dat");
        assert!(t.is_ancestor_or_self(1, 6));
        assert!(!t.is_ancestor_or_self(2, 1));
    }

    #[test]
    fn out_of_order_put() {
        let mut b = TreeBuilder::new();
        b.resize(3);
        b.put(2, 1, "f", file(7, 7, 0));
        b.put(1, 0, "d", dir());
        b.put(0, NONE, "R", dir());
        let t = b.finish();
        assert_eq!(t.node(0).size, 7);
        assert_eq!(t.path(2), "R\\d\\f");
    }
}
