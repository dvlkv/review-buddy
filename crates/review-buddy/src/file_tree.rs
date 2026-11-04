//! Builds the file tree's rows (folders first, single-child chains compressed,
//! "Review first" section) from diff paths.

use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTreeRow {
    pub id: String,
    pub depth: usize,
    pub name: String,
    pub is_folder: bool,
    pub is_section: bool,
    pub file_index: Option<usize>,
    /// 1-based position in the "Review first" list.
    pub rank: Option<usize>,
}

#[derive(Default)]
struct Node {
    name: String,
    children: HashMap<String, Node>,
    file_index: Option<usize>,
}

impl Node {
    fn child(&mut self, n: &str) -> &mut Node {
        self.children.entry(n.to_string()).or_insert_with(|| Node { name: n.to_string(), ..Default::default() })
    }
}

pub struct FileTreeBuilder;

impl FileTreeBuilder {
    /// File indices in display order: `priority` first, then the rest in tree order.
    pub fn display_order(paths: &[String], priority: &[usize]) -> Vec<usize> {
        let excl: HashSet<usize> = priority.iter().copied().collect();
        let mut out = priority.to_vec();
        out.extend(Self::tree_rows(paths, "", &HashSet::new(), &excl).into_iter().filter_map(|r| r.file_index));
        out
    }

    /// Visible rows. `collapsed` holds folder ids; `filter` is a case-insensitive substring.
    pub fn rows(paths: &[String], filter: &str, collapsed: &HashSet<String>, priority: &[usize]) -> Vec<FileTreeRow> {
        let q = filter.to_lowercase();
        if priority.is_empty() {
            return Self::tree_rows(paths, &q, collapsed, &HashSet::new());
        }
        let section = |id: &str, name: &str| FileTreeRow {
            id: id.into(),
            depth: 0,
            name: name.into(),
            is_folder: false,
            is_section: true,
            file_index: None,
            rank: None,
        };
        let mut out = vec![section("§first", "Review first")];
        for (n, &i) in priority.iter().enumerate() {
            if !q.is_empty() && !paths[i].to_lowercase().contains(&q) {
                continue;
            }
            out.push(FileTreeRow {
                id: format!("§first/{}", paths[i]),
                depth: 0,
                name: paths[i].rsplit('/').next().unwrap_or(&paths[i]).to_string(),
                is_folder: false,
                is_section: false,
                file_index: Some(i),
                rank: Some(n + 1),
            });
        }
        out.push(section("§all", "All files"));
        let excl: HashSet<usize> = priority.iter().copied().collect();
        out.extend(Self::tree_rows(paths, &q, collapsed, &excl));
        out
    }

    fn tree_rows(paths: &[String], q: &str, collapsed: &HashSet<String>, excluding: &HashSet<usize>) -> Vec<FileTreeRow> {
        let mut root = Node::default();
        for (i, p) in paths.iter().enumerate() {
            if excluding.contains(&i) || (!q.is_empty() && !p.to_lowercase().contains(q)) {
                continue;
            }
            let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
            let mut n = &mut root;
            if parts.len() > 1 {
                for part in &parts[..parts.len() - 1] {
                    n = n.child(&format!("{part}/"));
                }
            }
            n.child(parts.last().copied().unwrap_or(p)).file_index = Some(i);
        }
        let mut out = vec![];
        fn walk(node: &Node, depth: usize, prefix: &str, collapsed: &HashSet<String>, out: &mut Vec<FileTreeRow>) {
            let mut folders: Vec<&Node> = node.children.values().filter(|c| c.file_index.is_none()).collect();
            let mut files: Vec<&Node> = node.children.values().filter(|c| c.file_index.is_some()).collect();
            folders.sort_by_key(|n| n.name.to_lowercase());
            files.sort_by_key(|n| n.name.to_lowercase());
            for f in folders {
                let mut f = f;
                let mut name = f.name.clone();
                // Compress single-child folder chains.
                while f.children.len() == 1 {
                    let only = f.children.values().next().unwrap();
                    if only.file_index.is_some() {
                        break;
                    }
                    name.push_str(&only.name);
                    f = only;
                }
                let id = format!("{prefix}{name}");
                out.push(FileTreeRow {
                    id: id.clone(),
                    depth,
                    name: name.trim_end_matches('/').to_string(),
                    is_folder: true,
                    is_section: false,
                    file_index: None,
                    rank: None,
                });
                if !collapsed.contains(&id) {
                    walk(f, depth + 1, &id, collapsed, out);
                }
            }
            for f in files {
                out.push(FileTreeRow {
                    id: format!("{prefix}{}", f.name),
                    depth,
                    name: f.name.clone(),
                    is_folder: false,
                    is_section: false,
                    file_index: f.file_index,
                    rank: None,
                });
            }
        }
        walk(&root, 0, "", collapsed, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn folders_first_and_compressed() {
        let paths = p(&["z.txt", "src/a/b/c.rs", "src/a/b/d.rs", "README.md", "lib/x.go"]);
        let rows = FileTreeBuilder::rows(&paths, "", &HashSet::new(), &[]);
        let names: Vec<_> = rows.iter().map(|r| (r.depth, r.name.as_str())).collect();
        assert_eq!(
            names,
            vec![(0, "lib"), (1, "x.go"), (0, "src/a/b"), (1, "c.rs"), (1, "d.rs"), (0, "README.md"), (0, "z.txt")]
        );
        assert_eq!(FileTreeBuilder::display_order(&paths, &[]), vec![4, 1, 2, 3, 0]);
        assert_eq!(FileTreeBuilder::display_order(&paths, &[3]), vec![3, 4, 1, 2, 0]);
    }

    #[test]
    fn collapse_and_filter_and_priority() {
        let paths = p(&["src/a.rs", "src/b.rs", "docs/x.md"]);
        let mut c = HashSet::new();
        c.insert("src/".to_string());
        let rows = FileTreeBuilder::rows(&paths, "", &c, &[]);
        assert_eq!(rows.len(), 3); // docs, x.md, src (collapsed)
        let rows = FileTreeBuilder::rows(&paths, "B.RS", &HashSet::new(), &[]);
        assert_eq!(rows.iter().filter(|r| !r.is_folder).count(), 1);
        let rows = FileTreeBuilder::rows(&paths, "", &HashSet::new(), &[2]);
        assert!(rows[0].is_section);
        assert_eq!(rows[1].file_index, Some(2));
        assert_eq!(rows[1].rank, Some(1));
        assert_eq!(rows[2].name, "All files");
    }
}
