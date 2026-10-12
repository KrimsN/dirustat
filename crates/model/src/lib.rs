//! L0: модель дерева файлов.
//!
//! Дерево хранится в плоском arena-векторе: узел ссылается на родителя, первого потомка и
//! следующего брата по индексу `NodeId`. Родитель всегда добавляется раньше потомка, поэтому
//! агрегация размеров идёт одним обратным проходом без рекурсии (миллионы узлов, глубокие деревья).

use std::fmt;

/// Максимум узлов: `NodeId` — это `u32`, плюс запас под служебные значения.
pub const MAX_NODES: usize = (u32::MAX - 1) as usize;

/// Индекс узла в arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    /// Индекс узла как `usize` для доступа к arena.
    pub fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

/// Тип узла дерева.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    /// Симлинк/junction/reparse point: размер собственный, внутрь не заходим.
    Link,
}

/// Узел дерева. Поля приватны: инварианты поддерживает `Tree`.
#[derive(Clone, Debug)]
pub struct Node {
    name: Box<str>,
    kind: NodeKind,
    own_size: u64,
    total_size: u64,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    next_sibling: Option<NodeId>,
}

impl Node {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn kind(&self) -> NodeKind {
        self.kind
    }
    /// Размер самого узла (для каталога обычно 0).
    pub fn own_size(&self) -> u64 {
        self.own_size
    }
    /// Размер узла вместе с потомками; корректен после `Tree::aggregate`.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
}

/// Ошибки модели. Паники в этом крейте запрещены.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// Такого `NodeId` нет в дереве.
    UnknownNode,
    /// Потомка нельзя добавить к файлу или ссылке.
    NotADirectory,
    /// Превышен `MAX_NODES`.
    TooManyNodes,
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNode => f.write_str("unknown node id"),
            Self::NotADirectory => f.write_str("parent is not a directory"),
            Self::TooManyNodes => f.write_str("node limit exceeded"),
        }
    }
}

impl std::error::Error for ModelError {}

/// Дерево файлов. Корень всегда существует и всегда каталог.
#[derive(Clone, Debug)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    /// Создаёт дерево с корневым каталогом `root_name` (обычно путь, который сканируем).
    pub fn new(root_name: &str) -> Self {
        let root = Node {
            name: root_name.into(),
            kind: NodeKind::Dir,
            own_size: 0,
            total_size: 0,
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
        };
        Self { nodes: vec![root] }
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Дерево никогда не пусто (есть корень); метод нужен для clippy и симметрии с `len`.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    /// Добавляет потомка. Размер `own_size` учитывается в итогах после `aggregate`.
    pub fn add_child(
        &mut self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        own_size: u64,
    ) -> Result<NodeId, ModelError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(ModelError::TooManyNodes);
        }
        let parent_node = self.nodes.get(parent.index()).ok_or(ModelError::UnknownNode)?;
        if parent_node.kind != NodeKind::Dir {
            return Err(ModelError::NotADirectory);
        }
        let prev_last = parent_node.last_child;
        let new_id = NodeId(u32::try_from(self.nodes.len()).map_err(|_| ModelError::TooManyNodes)?);
        self.nodes.push(Node {
            name: name.into(),
            kind,
            own_size,
            total_size: own_size,
            parent: Some(parent),
            first_child: None,
            last_child: None,
            next_sibling: None,
        });
        if let Some(prev) = prev_last.and_then(|p| self.nodes.get_mut(p.index())) {
            prev.next_sibling = Some(new_id);
        }
        if let Some(p) = self.nodes.get_mut(parent.index()) {
            if p.first_child.is_none() {
                p.first_child = Some(new_id);
            }
            p.last_child = Some(new_id);
        }
        Ok(new_id)
    }

    /// Итерация по прямым потомкам.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children { tree: self, next: self.get(id).and_then(|n| n.first_child) }
    }

    /// Пересчитывает `total_size` всех узлов одним обратным проходом (родитель раньше потомка).
    /// Суммы насыщающие: враждебные размеры не вызывают переполнения.
    pub fn aggregate(&mut self) {
        for node in &mut self.nodes {
            node.total_size = node.own_size;
        }
        for idx in (1..self.nodes.len()).rev() {
            let Some((child_total, parent)) = self.nodes.get(idx).and_then(|n| n.parent.map(|p| (n.total_size, p)))
            else {
                continue;
            };
            if let Some(p) = self.nodes.get_mut(parent.index()) {
                p.total_size = p.total_size.saturating_add(child_total);
            }
        }
    }
}

/// Итератор по прямым потомкам узла в порядке добавления.
#[derive(Debug)]
pub struct Children<'a> {
    tree: &'a Tree,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.next?;
        self.next = self.tree.get(current).and_then(|n| n.next_sibling);
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_sizes_bottom_up() {
        let mut t = Tree::new("C:\\");
        let dir = t.add_child(t.root(), "dir", NodeKind::Dir, 0).unwrap();
        t.add_child(dir, "a.bin", NodeKind::File, 100).unwrap();
        t.add_child(dir, "b.bin", NodeKind::File, 50).unwrap();
        t.add_child(t.root(), "c.bin", NodeKind::File, 7).unwrap();
        t.aggregate();
        assert_eq!(t.get(dir).unwrap().total_size(), 150);
        assert_eq!(t.get(t.root()).unwrap().total_size(), 157);
        assert_eq!(t.children(t.root()).count(), 2);
    }

    #[test]
    fn rejects_children_of_files_and_unknown_parents() {
        let mut t = Tree::new("r");
        let f = t.add_child(t.root(), "f", NodeKind::File, 1).unwrap();
        assert_eq!(t.add_child(f, "x", NodeKind::File, 1), Err(ModelError::NotADirectory));
        let bogus = NodeId(999);
        assert_eq!(t.add_child(bogus, "x", NodeKind::File, 1), Err(ModelError::UnknownNode));
    }

    #[test]
    fn hostile_sizes_saturate() {
        let mut t = Tree::new("r");
        t.add_child(t.root(), "a", NodeKind::File, u64::MAX).unwrap();
        t.add_child(t.root(), "b", NodeKind::File, u64::MAX).unwrap();
        t.aggregate();
        assert_eq!(t.get(t.root()).unwrap().total_size(), u64::MAX);
    }
}
