//! [`Plan::next_eligible`] (single-step scheduler) and
//! [`Plan::topological_order`] (full ordering).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use petgraph::Direction;
use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use specify_error::Error;

use super::model::{Entry, Plan, Status};

impl Plan {
    /// First entry in list order whose dependencies are all `done` and
    /// whose own status is `pending`. Returns `None` when nothing is
    /// eligible (plan finished, blocked, empty) **or when any entry is
    /// currently `in-progress`** — the driver must not pick a new
    /// change while one is active. The in-progress check runs before
    /// any dependency walk, so this function is independent of
    /// [`Plan::topological_order`] and safe to call on cyclic plans.
    ///
    /// An unknown `depends_on` target is treated as "not done", so the
    /// entry is not eligible. Orphan-reference diagnostics belong to
    /// [`Plan::validate`].
    #[must_use]
    pub fn next_eligible(&self) -> Option<&Entry> {
        if self.entries.iter().any(|c| c.status == Status::InProgress) {
            return None;
        }
        let status_by_name: HashMap<&str, Status> =
            self.entries.iter().map(|c| (c.name.as_str(), c.status)).collect();
        self.entries.iter().find(|c| {
            c.status == Status::Pending
                && c.depends_on
                    .iter()
                    .all(|dep| status_by_name.get(dep.as_str()).copied() == Some(Status::Done))
        })
    }

    /// Entries in dependency-respecting order. Errors with an
    /// `Error::Diag` describing the cycle when the `depends_on` graph
    /// contains one.
    ///
    /// Tie-break rule: when two entries are simultaneously "ready"
    /// (dependencies already emitted), the one earlier in
    /// [`Plan::entries`] wins. This makes the output deterministic and
    /// a pure function of list order.
    ///
    /// Unknown `depends_on` targets are treated as satisfied for
    /// ordering purposes so orphan references cannot deadlock the sort;
    /// surfacing them is [`Plan::validate`]'s job.
    ///
    /// Implementation: we build a `DiGraph`, use `petgraph::toposort`
    /// (plus `tarjan_scc` on failure) for cycle detection and
    /// offender-naming, then walk the graph via a priority-queue Kahn
    /// where the priority is the original `NodeIndex` (which equals
    /// each entry's list position, since we insert in list order).
    /// That keeps the list-order tie-break contract while dropping
    /// the old O(n²) "sweep until fixpoint" fallback.
    ///
    /// # Panics
    ///
    /// Panics if the internal indegree map is inconsistent (should never
    /// happen in practice since every node is inserted during init).
    ///
    /// # Errors
    ///
    /// Errors with `Error::Diag` when the dependency graph has a cycle.
    pub fn topological_order(&self) -> Result<Vec<&Entry>, Error> {
        let mut graph: DiGraph<&str, ()> = DiGraph::new();
        let mut idx = HashMap::new();
        for entry in &self.entries {
            let node = graph.add_node(entry.name.as_str());
            idx.insert(entry.name.as_str(), node);
        }
        for entry in &self.entries {
            let to = idx[entry.name.as_str()];
            for dep in &entry.depends_on {
                if let Some(&from) = idx.get(dep.as_str()) {
                    graph.add_edge(from, to, ());
                }
            }
        }

        if toposort(&graph, None).is_err() {
            let offender = tarjan_scc(&graph)
                .into_iter()
                .find(|scc| {
                    scc.len() > 1 || (scc.len() == 1 && graph.find_edge(scc[0], scc[0]).is_some())
                })
                .map_or_else(|| "<unknown>".to_string(), |scc| graph[scc[0]].to_string());
            return Err(Error::Diag {
                code: "plan-dependency-cycle",
                detail: format!("plan has dependency cycle involving '{offender}'"),
            });
        }

        let mut indegree: HashMap<NodeIndex, usize> = graph
            .node_indices()
            .map(|n| (n, graph.neighbors_directed(n, Direction::Incoming).count()))
            .collect();
        let mut ready: BinaryHeap<Reverse<NodeIndex>> =
            indegree.iter().filter_map(|(&n, &d)| (d == 0).then_some(Reverse(n))).collect();

        let mut rank: HashMap<NodeIndex, usize> = HashMap::with_capacity(self.entries.len());
        let mut next_rank = 0_usize;
        while let Some(Reverse(node)) = ready.pop() {
            rank.insert(node, next_rank);
            next_rank += 1;
            for downstream in graph.neighbors_directed(node, Direction::Outgoing) {
                let entry = indegree.get_mut(&downstream).expect("indegree init covers every node");
                *entry -= 1;
                if *entry == 0 {
                    ready.push(Reverse(downstream));
                }
            }
        }

        let mut output: Vec<&Entry> = self.entries.iter().collect();
        output.sort_by_key(|c| rank[&idx[c.name.as_str()]]);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        RFC_EXAMPLE_YAML, change, change_with_deps, plan_with_changes,
    };
    use super::*;

    #[test]
    fn next_eligible_picks_first_ready() {
        let plan = plan_with_changes(vec![
            change("a", Status::Done),
            change("b", Status::Done),
            change_with_deps("c", Status::Pending, &["b"]),
        ]);
        let eligible = plan.next_eligible().expect("c should be eligible");
        assert_eq!(eligible.name, "c");
    }

    #[test]
    fn next_eligible_skips_unmet_deps() {
        let plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let eligible = plan.next_eligible().expect("a should be eligible");
        assert_eq!(eligible.name, "a", "b's dep 'a' is not done, so a (no deps) wins");
    }

    #[test]
    fn next_eligible_blocked_by_in_progress() {
        let plan =
            plan_with_changes(vec![change("a", Status::InProgress), change("b", Status::Pending)]);
        assert!(
            plan.next_eligible().is_none(),
            "an in-progress entry must block any new selection"
        );
    }

    #[test]
    fn next_eligible_none_when_finished() {
        let plan = plan_with_changes(vec![
            change("a", Status::Done),
            change("b", Status::Skipped),
            change("c", Status::Failed),
        ]);
        assert!(plan.next_eligible().is_none());
    }

    #[test]
    fn next_eligible_tiebreak() {
        let plan = plan_with_changes(vec![
            change("alpha", Status::Pending),
            change("beta", Status::Pending),
        ]);
        let eligible = plan.next_eligible().expect("alpha should be first");
        assert_eq!(eligible.name, "alpha", "list-order tie-break must pick the first entry");
    }

    /// Drive `next_eligible` forward across the reference example plan,
    /// marking each returned entry `done`, and assert the exact
    /// traversal sequence.
    #[test]
    fn next_eligible_rfc_forward() {
        let mut plan: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        for entry in &mut plan.entries {
            entry.status = Status::Pending;
            entry.status_reason = None;
        }

        let mut traversal = Vec::new();
        while let Some(next) = plan.next_eligible() {
            let name = next.name.clone();
            traversal.push(name.clone());
            let entry = plan
                .entries
                .iter_mut()
                .find(|c| c.name == name)
                .expect("returned name must exist in plan");
            entry.status = Status::Done;
        }

        let expected = [
            "user-registration",
            "email-verification",
            "registration-duplicate-email-crash",
            "notification-preferences",
            "extract-shared-validation",
            "product-catalog",
            "shopping-cart",
            "checkout-api",
            "checkout-ui",
        ];
        assert_eq!(
            traversal, expected,
            "next_eligible traversal should follow the §The Plan reference forward order"
        );
    }

    #[test]
    fn next_eligible_blocks_mid_cycle() {
        let plan = plan_with_changes(vec![
            change("in-flight", Status::InProgress),
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        assert!(
            plan.next_eligible().is_none(),
            "in-progress entry must block selection before any dependency walk"
        );
    }

    #[test]
    fn topo_order_rfc_example() {
        let plan: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let ordered: Vec<&str> = plan
            .topological_order()
            .expect("rfc plan has no cycles")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let expected = [
            "user-registration",
            "email-verification",
            "registration-duplicate-email-crash",
            "notification-preferences",
            "extract-shared-validation",
            "product-catalog",
            "shopping-cart",
            "checkout-api",
            "checkout-ui",
        ];
        assert_eq!(
            ordered, expected,
            "topological_order should match next_eligible forward traversal"
        );
    }

    #[test]
    fn topo_order_cycle_errors() {
        let plan = plan_with_changes(vec![
            change_with_deps("a", Status::Pending, &["c"]),
            change_with_deps("b", Status::Pending, &["a"]),
            change_with_deps("c", Status::Pending, &["b"]),
        ]);
        let err = plan.topological_order().expect_err("cycle must surface as Err");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-dependency-cycle");
                assert!(
                    detail.contains("cycle"),
                    "Diag detail should mention 'cycle', got: {detail}"
                );
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
    }

    #[test]
    fn topo_order_deterministic_tiebreak() {
        let alpha_first = plan_with_changes(vec![
            change("alpha", Status::Pending),
            change("beta", Status::Pending),
        ]);
        let order: Vec<&str> = alpha_first
            .topological_order()
            .expect("no cycle")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(order, ["alpha", "beta"]);

        let beta_first = plan_with_changes(vec![
            change("beta", Status::Pending),
            change("alpha", Status::Pending),
        ]);
        let order: Vec<&str> = beta_first
            .topological_order()
            .expect("no cycle")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            order,
            ["beta", "alpha"],
            "swapping list order must swap topo order when no deps constrain it"
        );
    }

    /// `next_eligible` must not depend on `topological_order` succeeding:
    /// even when the plan has a cycle, an in-progress entry short-circuits
    /// selection to `None` without walking the dependency graph.
    #[test]
    fn next_eligible_with_cycle() {
        let plan = plan_with_changes(vec![
            change("busy", Status::InProgress),
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        assert!(plan.next_eligible().is_none());
        assert!(plan.topological_order().is_err(), "cycle should surface from topological_order");
    }
}
