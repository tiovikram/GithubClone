//! Relay-style connections with real, opaque, offset-based cursors.
use async_graphql::Object;
use base64::Engine;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Opaque cursor for position `i` (0-based) in a result set (GitHub's look: `Y3Vyc29yOnYyOpHO...`).
pub fn cursor(i: usize) -> String {
    B64.encode(format!("cursor:v2:{i}"))
}

pub fn decode_cursor(c: &str) -> Option<usize> {
    let raw = B64.decode(c.trim()).ok()?;
    let s = String::from_utf8(raw).ok()?;
    s.strip_prefix("cursor:v2:")?.parse().ok()
}

/// first/after/last/before as given by the client.
#[derive(Clone, Debug, Default)]
pub struct ConnArgs {
    pub first: Option<i32>,
    pub after: Option<String>,
    pub last: Option<i32>,
    pub before: Option<String>,
}

impl ConnArgs {
    pub fn new(first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> Self {
        ConnArgs { first, after, last, before }
    }

    /// Computes the [start, end) slice of a result set of `total` items.
    pub fn window(&self, total: usize) -> Window {
        let lower = self
            .after
            .as_deref()
            .and_then(decode_cursor)
            .map(|i| (i + 1).min(total))
            .unwrap_or(0);
        let upper = self.before.as_deref().and_then(decode_cursor).map(|i| i.min(total)).unwrap_or(total).max(lower);
        let mut start = lower;
        let mut end = upper;
        if let Some(f) = self.first {
            end = end.min(start + f.clamp(0, 100) as usize);
        }
        if let Some(l) = self.last {
            start = start.max(end.saturating_sub(l.clamp(0, 100) as usize));
        }
        if self.first.is_none() && self.last.is_none() {
            // GitHub requires first/last; be forgiving and cap at 100.
            end = end.min(start + 100);
        }
        Window { start, end, has_next: end < total, has_prev: start > 0, total }
    }

    /// GitHub's search treats `last: N` without `before` like `first: N` (that is how gh uses it).
    pub fn search_normalized(mut self) -> Self {
        if self.first.is_none() && self.before.is_none() && self.last.is_some() {
            self.first = self.last.take();
        }
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub start: usize,
    pub end: usize,
    pub has_next: bool,
    pub has_prev: bool,
    pub total: usize,
}

impl Window {
    pub fn offset(&self) -> i64 {
        self.start as i64
    }
    pub fn limit(&self) -> i64 {
        (self.end - self.start) as i64
    }
    /// Slices an in-memory list according to the window.
    pub fn slice<T: Clone>(&self, all: &[T]) -> Vec<T> {
        all.get(self.start..self.end.min(all.len())).map(|s| s.to_vec()).unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub struct PageInfo {
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
}

impl PageInfo {
    pub fn from_window(w: &Window, count: usize) -> Self {
        PageInfo {
            start_cursor: (count > 0).then(|| cursor(w.start)),
            end_cursor: (count > 0).then(|| cursor(w.start + count - 1)),
            has_next_page: w.has_next,
            has_previous_page: w.has_prev,
        }
    }
    pub fn empty() -> Self {
        PageInfo { start_cursor: None, end_cursor: None, has_next_page: false, has_previous_page: false }
    }
}

#[Object]
impl PageInfo {
    async fn start_cursor(&self) -> Option<&str> {
        self.start_cursor.as_deref()
    }
    async fn end_cursor(&self) -> Option<&str> {
        self.end_cursor.as_deref()
    }
    async fn has_next_page(&self) -> bool {
        self.has_next_page
    }
    async fn has_previous_page(&self) -> bool {
        self.has_previous_page
    }
}

/// Defines `<Name>Connection` / `<Name>Edge` GraphQL types for a node type.
#[macro_export]
macro_rules! gql_connection {
    ($conn:ident, $edge:ident, $conn_name:literal, $edge_name:literal, $node:ty) => {
        #[derive(Clone)]
        pub struct $conn {
            pub items: Vec<$node>,
            pub window: $crate::graphql::connection::Window,
        }

        impl $conn {
            /// Builds a connection from the already-sliced items of `window`.
            pub fn new(items: Vec<$node>, window: $crate::graphql::connection::Window) -> Self {
                $conn { items, window }
            }
            /// Builds a connection from the complete result list.
            pub fn from_all(all: Vec<$node>, args: &$crate::graphql::connection::ConnArgs) -> Self {
                let window = args.window(all.len());
                let items = window.slice(&all);
                $conn { items, window }
            }
            pub fn empty() -> Self {
                Self::from_all(Vec::new(), &Default::default())
            }
        }

        #[async_graphql::Object(name = $conn_name)]
        impl $conn {
            async fn total_count(&self) -> i32 {
                self.window.total as i32
            }
            async fn nodes(&self) -> Vec<Option<$node>> {
                self.items.iter().cloned().map(Some).collect()
            }
            async fn edges(&self) -> Vec<Option<$edge>> {
                self.items
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        Some($edge { cursor: $crate::graphql::connection::cursor(self.window.start + i), node: n.clone() })
                    })
                    .collect()
            }
            async fn page_info(&self) -> $crate::graphql::connection::PageInfo {
                $crate::graphql::connection::PageInfo::from_window(&self.window, self.items.len())
            }
        }

        #[derive(Clone)]
        pub struct $edge {
            pub cursor: String,
            pub node: $node,
        }

        #[async_graphql::Object(name = $edge_name)]
        impl $edge {
            async fn cursor(&self) -> &str {
                &self.cursor
            }
            async fn node(&self) -> Option<$node> {
                Some(self.node.clone())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows() {
        let a = ConnArgs::new(Some(2), None, None, None);
        let w = a.window(5);
        assert_eq!((w.start, w.end, w.has_next), (0, 2, true));
        let a = ConnArgs::new(Some(2), Some(cursor(1)), None, None);
        let w = a.window(5);
        assert_eq!((w.start, w.end, w.has_next, w.has_prev), (2, 4, true, true));
        let a = ConnArgs::new(None, None, Some(1), None);
        let w = a.window(5);
        assert_eq!((w.start, w.end), (4, 5));
        let a = ConnArgs::new(None, None, Some(2), Some(cursor(3)));
        let w = a.window(5);
        assert_eq!((w.start, w.end), (1, 3));
        assert_eq!(decode_cursor(&cursor(42)), Some(42));
    }
}
