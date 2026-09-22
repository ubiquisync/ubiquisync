impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> List<Id, T> {
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.iter_impl(|n| !n.effect_deleted)
    }

    pub fn iter_prepare(&self) -> impl Iterator<Item = &T> {
        self.iter_impl(|n| n.prepare_state == PrepareState::Inserted)
    }

    fn iter_impl<F>(&self, is_inserted: F) -> impl Iterator<Item = &T>
    where
        F: Fn(&Node<Id, T>) -> bool,
    {
        // we make a few optimizations here to limit memory use:
        // - push the next tree walk operation onto the stack rather than iterating through everything at once
        // - use a smallvec to avoid heap allocation for shallow trees
        enum Todo<'b, Id, T> {
            Traverse(&'b Node<Id, T>),
            TraverseSiblings(&'b [NodeRef<Id>], usize),
            Yield(&'b Node<Id, T>),
        }
        let mut stack: SmallVec<[Todo<'_, Id, T>; 16]> = smallvec![Todo::Traverse(&self.root)];
        std::iter::from_fn(move || {
            loop {
                let top = stack.pop()?;
                match top {
                    Todo::Traverse(node) => {
                        stack.push(Todo::Yield(node));
                        if !node.left_children.is_empty() {
                            stack.push(Todo::TraverseSiblings(&node.left_children, 0));
                        }
                    }
                    Todo::TraverseSiblings(element_ids, idx) => {
                        let n = element_ids.len();
                        if idx < n {
                            if idx + 1 < n {
                                stack.push(Todo::TraverseSiblings(element_ids, idx + 1));
                            }

                            stack.push(Todo::Traverse(&self.nodes[element_ids[idx].index.0]));
                        }
                    }
                    Todo::Yield(node) => {
                        if !node.right_children.is_empty() {
                            stack.push(Todo::TraverseSiblings(&node.right_children, 0));
                        }
                        if let Some(ref content) = node.content
                            && is_inserted(node)
                        {
                            return Some(content);
                        }
                    }
                }
            }
        })
    }
}
