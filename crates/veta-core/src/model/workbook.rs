use std::fmt;

use super::Document;

/// Stable identifier of an open document. Never reused within a workbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(u64);

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// The set of open documents, in tab order.
#[derive(Debug, Default)]
pub struct Workbook {
    documents: Vec<(DocumentId, Document)>,
    next_id: u64,
}

impl Workbook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a document at the end and returns its id.
    pub fn add(&mut self, document: Document) -> DocumentId {
        let id = DocumentId(self.next_id);
        self.next_id += 1;
        self.documents.push((id, document));
        id
    }

    /// Removes a document and returns it.
    pub fn close(&mut self, id: DocumentId) -> Option<Document> {
        let index = self.documents.iter().position(|(i, _)| *i == id)?;
        Some(self.documents.remove(index).1)
    }

    pub fn get(&self, id: DocumentId) -> Option<&Document> {
        self.documents
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, d)| d)
    }

    pub fn get_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.documents
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, d)| d)
    }

    /// Documents in tab order.
    pub fn iter(&self) -> impl Iterator<Item = (DocumentId, &Document)> {
        self.documents.iter().map(|(id, doc)| (*id, doc))
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_not_reused() {
        let mut wb = Workbook::new();
        let a = wb.add(Document::new());
        let b = wb.add(Document::new());
        assert_ne!(a, b);

        assert!(wb.close(a).is_some());
        let c = wb.add(Document::new());
        assert_ne!(a, c);
        assert_eq!(wb.iter().map(|(id, _)| id).collect::<Vec<_>>(), [b, c]);
    }

    #[test]
    fn close_unknown_document_returns_none() {
        let mut wb = Workbook::new();
        let a = wb.add(Document::new());
        wb.close(a);
        assert!(wb.close(a).is_none());
        assert!(wb.is_empty());
    }
}
