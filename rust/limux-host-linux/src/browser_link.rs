pub fn open_in_right_pane<Pane>(
    source: &Pane,
    right: Option<&Pane>,
    split_right: impl FnOnce() -> bool,
    mut append_browser_tab: impl FnMut(&Pane),
) {
    if let Some(right) = right {
        append_browser_tab(right);
    } else if !split_right() {
        append_browser_tab(source);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::open_in_right_pane;

    #[derive(Default)]
    struct TestPane {
        pages: RefCell<Vec<&'static str>>,
        save_requests: Cell<usize>,
    }

    impl TestPane {
        fn with_pages(pages: &[&'static str]) -> Self {
            Self {
                pages: RefCell::new(pages.to_vec()),
                save_requests: Cell::new(0),
            }
        }

        fn append_page(&self, url: &'static str) {
            self.pages.borrow_mut().push(url);
            self.save_requests.set(self.save_requests.get() + 1);
        }
    }

    #[test]
    fn existing_right_pane_gets_a_fresh_tab_and_preserves_existing_pages() {
        let source = TestPane::default();
        let right = TestPane::with_pages(&["https://existing.example"]);
        let split_attempted = Cell::new(false);

        open_in_right_pane(
            &source,
            Some(&right),
            || {
                split_attempted.set(true);
                true
            },
            |pane| pane.append_page("https://new.example"),
        );

        assert_eq!(
            right.pages.borrow().as_slice(),
            ["https://existing.example", "https://new.example"]
        );
        assert_eq!(right.save_requests.get(), 1);
        assert!(source.pages.borrow().is_empty());
        assert!(!split_attempted.get());
    }

    #[test]
    fn missing_right_pane_creates_and_persists_a_browser_split() {
        let source = TestPane::default();
        let created_pages = RefCell::new(Vec::new());
        let save_requests = Cell::new(0);

        open_in_right_pane(
            &source,
            None,
            || {
                created_pages.borrow_mut().push("https://new.example");
                save_requests.set(save_requests.get() + 1);
                true
            },
            |pane| pane.append_page("https://new.example"),
        );

        assert_eq!(created_pages.borrow().as_slice(), ["https://new.example"]);
        assert_eq!(save_requests.get(), 1);
        assert!(source.pages.borrow().is_empty());
        assert_eq!(source.save_requests.get(), 0);
    }

    #[test]
    fn unavailable_split_falls_back_to_a_persisted_source_pane_tab() {
        let source = TestPane::with_pages(&["https://existing.example"]);
        let split_attempted = Cell::new(false);

        open_in_right_pane(
            &source,
            None,
            || {
                split_attempted.set(true);
                false
            },
            |pane| pane.append_page("https://new.example"),
        );

        assert!(split_attempted.get());
        assert_eq!(
            source.pages.borrow().as_slice(),
            ["https://existing.example", "https://new.example"]
        );
        assert_eq!(source.save_requests.get(), 1);
    }
}
