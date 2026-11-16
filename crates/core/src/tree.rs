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
    /// Removido por uma ação do usuário (ver `Tree::remove_subtree`).
    pub const REMOVED: u16 = 1 << 8;
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

#[derive(Clone, Debug, Default)]
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

    /// Desce a partir de `from` por nomes de componentes (sem diferenciar maiúsculas).
    pub fn find_child_path(&self, from: NodeId, components: &[&str]) -> Option<NodeId> {
        let mut cur = from;
        for comp in components {
            cur = self.children(cur).iter().copied().find(|&c| {
                self.name(c).eq_ignore_ascii_case(comp)
                    || self.name(c).to_lowercase() == comp.to_lowercase()
            })?;
        }
        Some(cur)
    }

    /// Nó correspondente a um caminho completo (sem diferenciar maiúsculas), a partir do nome da
    /// raiz (ex.: `C:\` ou `C:\Users\Ana`). `None` se estiver fora da árvore.
    pub fn find_by_path(&self, path: &str) -> Option<NodeId> {
        let root = self.name(Self::ROOT).trim_end_matches('\\');
        let p = path.trim_end_matches('\\');
        if !p.get(..root.len())?.eq_ignore_ascii_case(root) {
            return None;
        }
        let rest = &p[root.len()..];
        if rest.is_empty() {
            return Some(Self::ROOT);
        }
        if !rest.starts_with('\\') {
            return None;
        }
        let comps: Vec<&str> = rest.split('\\').filter(|c| !c.is_empty()).collect();
        self.find_child_path(Self::ROOT, &comps)
    }

    /// Copia a subárvore de `id` para uma nova árvore (o nó `id` vira a raiz, nomeado `root_name`).
    pub fn extract_subtree(&self, id: NodeId, root_name: &str) -> Tree {
        let mut b = TreeBuilder::new();
        let n = self.node(id);
        let info = |n: &Node| NodeInfo {
            flags: n.flags,
            size: n.size,
            allocated: n.allocated,
            mtime: n.mtime,
        };
        // Arquivos mantêm seus tamanhos; diretórios serão reagregados em `finish`.
        let root = b.push(NONE, root_name, info(n));
        let mut stack = vec![(id, root)];
        while let Some((old, new)) = stack.pop() {
            for &c in self.children(old) {
                let cn = self.node(c);
                let nid = b.push(new, self.name(c), info(cn));
                if cn.is_dir() {
                    stack.push((c, nid));
                }
            }
        }
        b.finish()
    }

    /// Nó removido por [`Tree::remove_subtree`] (some das listagens e estatísticas).
    pub fn is_removed(&self, id: NodeId) -> bool {
        self.nodes[id as usize].flags & flags::REMOVED != 0
    }

    /// Soma `(dsize, dalloc, dfiles, ddirs)` (com sinal) em todos os ancestrais de `from`, inclusive.
    fn adjust_ancestors(&mut self, from: Option<NodeId>, d: (i128, i128, i64, i64)) {
        let mut cur = from;
        while let Some(id) = cur {
            let n = &mut self.nodes[id as usize];
            n.size = (i128::from(n.size) + d.0).max(0) as u64;
            n.allocated = (i128::from(n.allocated) + d.1).max(0) as u64;
            n.files = (i64::from(n.files) + d.2).max(0) as u32;
            n.dirs = (i64::from(n.dirs) + d.3).max(0) as u32;
            cur = (n.parent != NONE).then_some(n.parent);
        }
    }

    /// Remove `id` (e tudo abaixo dele) da árvore, corrigindo os agregados dos ancestrais.
    /// Os ids dos demais nós permanecem válidos. A raiz não pode ser removida.
    pub fn remove_subtree(&mut self, id: NodeId) -> bool {
        let Some(parent) = self.parent(id) else {
            return false;
        };
        if self.is_removed(id) {
            return false;
        }
        let n = self.nodes[id as usize];
        // Tira da lista de filhos do pai (troca com o último e encurta).
        let p = self.nodes[parent as usize];
        let range = p.child_start as usize..(p.child_start + p.child_count) as usize;
        if let Some(pos) = self.children[range.clone()].iter().position(|&c| c == id) {
            let last = range.end - 1;
            self.children.swap(range.start + pos, last);
            self.nodes[parent as usize].child_count -= 1;
        }
        // Marca a subárvore como removida.
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            self.nodes[x as usize].flags |= flags::REMOVED;
            let (s, c) = (
                self.nodes[x as usize].child_start,
                self.nodes[x as usize].child_count,
            );
            stack.extend_from_slice(&self.children[s as usize..(s + c) as usize]);
        }
        let ndirs = i64::from(n.dirs) + i64::from(n.is_dir());
        self.adjust_ancestors(
            Some(parent),
            (
                -i128::from(n.size),
                -i128::from(n.allocated),
                -i64::from(n.files),
                -ndirs,
            ),
        );
        true
    }

    /// Enxerta a árvore `sub` (a raiz dela vira filha de `parent`) e soma seus totais aos
    /// ancestrais. Devolve o id da nova raiz. `parent` deve ser um diretório.
    pub fn graft(&mut self, parent: NodeId, sub: &Tree) -> Option<NodeId> {
        if sub.is_empty() || !self.node(parent).is_dir() {
            return None;
        }
        let base = self.nodes.len() as u32;
        let names_base = self.names.len() as u32;
        let children_base = self.children.len() as u32;
        self.names.extend_from_slice(&sub.names);
        self.children.extend(sub.children.iter().map(|&c| c + base));
        self.nodes.extend(sub.nodes.iter().map(|n| Node {
            parent: if n.parent == NONE {
                parent
            } else {
                n.parent + base
            },
            name_off: n.name_off + names_base,
            child_start: n.child_start + children_base,
            ..*n
        }));
        let new_root = base;
        self.attach_child(parent, new_root);
        let r = sub.nodes[0];
        let ndirs = i64::from(r.dirs) + i64::from(r.is_dir());
        self.adjust_ancestors(
            Some(parent),
            (
                i128::from(r.size),
                i128::from(r.allocated),
                i64::from(r.files),
                ndirs,
            ),
        );
        let mtime = r.mtime;
        let mut cur = Some(parent);
        while let Some(x) = cur {
            let n = &mut self.nodes[x as usize];
            n.mtime = n.mtime.max(mtime);
            cur = (n.parent != NONE).then_some(n.parent);
        }
        Some(new_root)
    }

    /// Acrescenta `child` à lista de filhos de `parent` (realoca a lista no fim se preciso).
    fn attach_child(&mut self, parent: NodeId, child: NodeId) {
        let p = self.nodes[parent as usize];
        let end = (p.child_start + p.child_count) as usize;
        if end == self.children.len() {
            self.children.push(child);
        } else {
            let new_start = self.children.len() as u32;
            let old: Vec<NodeId> = self.children[p.child_start as usize..end].to_vec();
            self.children.extend(old);
            self.children.push(child);
            self.nodes[parent as usize].child_start = new_start;
        }
        self.nodes[parent as usize].child_count += 1;
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
    fn find_by_path_is_case_insensitive_and_bounded() {
        let t = sample();
        assert_eq!(t.find_by_path(r"c:\A\C\z.dat"), Some(5));
        assert_eq!(t.find_by_path(r"C:\"), Some(Tree::ROOT));
        assert_eq!(t.find_by_path(r"C:\a\"), Some(1));
        assert_eq!(t.find_by_path(r"D:\a"), None);
        assert_eq!(t.find_by_path(r"C:\nada"), None);
        assert_eq!(t.find_by_path("C:"), Some(Tree::ROOT));
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

    #[test]
    fn remove_subtree_fixes_ancestors() {
        let mut t = sample();
        // remove "c" (id 2): z.dat (10) + w.dat (20)
        assert!(t.remove_subtree(2));
        assert!(t.is_removed(2) && t.is_removed(5) && t.is_removed(6));
        let r = t.node(Tree::ROOT);
        assert_eq!((r.size, r.files, r.dirs), (1100, 2, 1));
        assert_eq!(t.node(1).size, 1000);
        assert_eq!(t.children(1).len(), 1); // só y.bin
        assert!(!t.remove_subtree(2), "remoção dupla");
        assert!(!t.remove_subtree(Tree::ROOT));
    }

    #[test]
    fn graft_adds_totals_and_keeps_old_ids() {
        let mut t = sample();
        let mut b = TreeBuilder::new();
        let r = b.push(NONE, "novo", dir());
        b.push(r, "n1.bin", file(500, 4096, 99));
        let sub_dir = b.push(r, "q", dir());
        b.push(sub_dir, "n2.bin", file(5, 4096, 3));
        let sub = b.finish();
        let id = t.graft(2, &sub).unwrap(); // dentro de "c"
        assert_eq!(t.name(id), "novo");
        assert_eq!(t.path(id), "C:\\a\\c\\novo");
        assert_eq!(t.parent(id), Some(2));
        assert_eq!(t.children(2).len(), 3);
        let r = t.node(Tree::ROOT);
        assert_eq!((r.size, r.files, r.dirs, r.mtime), (1635, 6, 4, 99));
        assert_eq!(t.node(2).size, 535);
        assert_eq!(
            t.path(5),
            "C:\\a\\c\\z.dat",
            "ids antigos continuam válidos"
        );
        // Enxerto na raiz (lista de filhos não está no fim do vetor).
        let mut b2 = TreeBuilder::new();
        b2.push(NONE, "solto.txt", file(1, 1, 0));
        t.graft(Tree::ROOT, &b2.finish()).unwrap();
        assert_eq!(t.children(Tree::ROOT).len(), 3);
        assert_eq!(t.node(Tree::ROOT).files, 7);
    }
}
