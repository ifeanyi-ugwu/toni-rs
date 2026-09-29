use super::Container;
use crate::error::SetupResult;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::sync::Arc;

pub(crate) struct DependencyGraph {
    container: Arc<RwLock<Container>>,
    module_token: String,
    visited: FxHashMap<String, bool>,
    temp_mark: FxHashMap<String, bool>,
    ordered: Vec<String>,
}

impl DependencyGraph {
    pub(crate) fn new(container: Arc<RwLock<Container>>, module_token: String) -> Self {
        Self {
            container,
            module_token,
            visited: FxHashMap::default(),
            temp_mark: FxHashMap::default(),
            ordered: Vec::new(),
        }
    }

    pub(crate) fn ordered_provider_tokens(mut self) -> SetupResult<Vec<String>> {
        let (providers, multi_providers) = {
            let container = self.container.read();
            let providers_map = container.provider_factories(&self.module_token)?;
            let providers = providers_map
                .iter()
                .map(|(token, provider)| (token.clone(), provider.dependency_tokens()))
                .collect::<Vec<(String, Vec<String>)>>();
            // Map from multi-collection base token (e.g. "PLUGINS") to the contributing
            // provider tokens within this module so the topological sort can treat all
            // contributors as implicit dependencies of any provider that injects the base token.
            let multi_providers: FxHashMap<String, Vec<String>> = container
                .multi_providers()
                .iter()
                .map(|(base, contribs)| {
                    let local: Vec<String> = contribs
                        .iter()
                        .filter(|(mt, _)| mt == &self.module_token)
                        .map(|(_, pt)| pt.clone())
                        .collect();
                    (base.clone(), local)
                })
                .collect();
            (providers, multi_providers)
        };
        let clone_providers = providers.clone();
        for (token, dependencies) in providers {
            if !self.visited.contains_key(&token) {
                self.visit_node(token, dependencies, &clone_providers, &multi_providers)?;
            }
        }
        Ok(self.ordered)
    }

    fn visit_node(
        &mut self,
        token: String,
        dependencies: Vec<String>,
        providers: &[(String, Vec<String>)],
        multi_providers: &FxHashMap<String, Vec<String>>,
    ) -> SetupResult {
        if self.temp_mark.contains_key(&token) {
            return Err(self.cycle_in_module(providers, multi_providers).into());
        }

        if self.visited.contains_key(&token) {
            return Ok(());
        }

        self.temp_mark.insert(token.clone(), true);

        for dep_token in &dependencies {
            if let Some((provider_token, provider_deps)) = providers
                .iter()
                .find(|(token, _)| token.as_str() == dep_token.as_str())
            {
                self.visit_node(
                    provider_token.clone(),
                    provider_deps.clone(),
                    providers,
                    multi_providers,
                )?;
            } else if let Some(contrib_tokens) = multi_providers.get(dep_token) {
                // dep_token is a multi-collection base token: visit all contributing
                // factories in this module first so the consumer is ordered after them.
                for contrib_token in contrib_tokens {
                    if let Some((_, contrib_deps)) =
                        providers.iter().find(|(t, _)| t == contrib_token)
                    {
                        self.visit_node(
                            contrib_token.clone(),
                            contrib_deps.clone(),
                            providers,
                            multi_providers,
                        )?;
                    }
                }
            }
        }

        self.temp_mark.remove(&token);
        self.visited.insert(token.clone(), true);
        self.ordered.push(token);
        Ok(())
    }

    /// The module's provider cycle in the form the cross-module stall uses, started at the provider
    /// declared first rather than the first in token order.
    fn cycle_in_module(
        &self,
        providers: &[(String, Vec<String>)],
        multi_providers: &FxHashMap<String, Vec<String>>,
    ) -> String {
        let adjacency: FxHashMap<String, Vec<String>> = providers
            .iter()
            .map(|(token, deps)| {
                let deps = deps
                    .iter()
                    .flat_map(|dep| match multi_providers.get(dep) {
                        Some(contributors) => contributors.clone(),
                        None => vec![dep.clone()],
                    })
                    .collect();
                (token.clone(), deps)
            })
            .collect();
        let declared = |token: &String| providers.iter().position(|(t, _)| t == token);
        let mut cycle = find_dependency_cycle(&adjacency).unwrap_or_default();
        cycle.pop();
        if let Some(first) = (0..cycle.len()).min_by_key(|&at| declared(&cycle[at])) {
            cycle.rotate_left(first);
        }
        if let Some(start) = cycle.first().cloned() {
            cycle.push(start);
        }
        cycle_message(&cycle, |_| Some(self.module_token.as_str()))
    }
}

/// Find one dependency cycle in a provider graph, if any exists.
///
/// `adjacency` maps each provider token to the tokens it depends on. Only tokens that
/// are themselves keys are treated as providers; a dependency on a value/factory token
/// or a token no module produces is a leaf and cannot be part of a cycle.
///
/// Returns the cycle as `[A, B, …, A]` — the entry token repeated at the end — or `None`
/// when the graph is acyclic. Roots are visited in sorted order so the result is
/// deterministic for a given graph. Used only on the failure path: by the per-module sort to
/// name the cycle it hit, and by the stall diagnostic to name one spanning modules.
pub(crate) fn find_dependency_cycle(
    adjacency: &FxHashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    enum Mark {
        OnStack,
        Done,
    }

    fn visit(
        node: &str,
        adjacency: &FxHashMap<String, Vec<String>>,
        marks: &mut FxHashMap<String, Mark>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        marks.insert(node.to_string(), Mark::OnStack);
        path.push(node.to_string());

        if let Some(deps) = adjacency.get(node) {
            for dep in deps {
                match marks.get(dep) {
                    Some(Mark::OnStack) => {
                        // Back-edge: slice the cycle out of the current DFS path.
                        let start = path.iter().position(|t| t == dep).unwrap();
                        let mut cycle = path[start..].to_vec();
                        cycle.push(dep.clone());
                        return Some(cycle);
                    }
                    Some(Mark::Done) => {}
                    None => {
                        if adjacency.contains_key(dep) {
                            if let Some(cycle) = visit(dep, adjacency, marks, path) {
                                return Some(cycle);
                            }
                        }
                    }
                }
            }
        }

        path.pop();
        marks.insert(node.to_string(), Mark::Done);
        None
    }

    let mut roots: Vec<&String> = adjacency.keys().collect();
    roots.sort();

    let mut marks: FxHashMap<String, Mark> = FxHashMap::default();
    for root in roots {
        if !marks.contains_key(root) {
            let mut path = Vec::new();
            if let Some(cycle) = visit(root, adjacency, &mut marks, &mut path) {
                return Some(cycle);
            }
        }
    }
    None
}

/// The refusal for a provider cycle, `cycle` naming its providers in order and closing on the first.
pub(crate) fn cycle_message<'a>(
    cycle: &'a [String],
    module_of: impl Fn(&str) -> Option<&'a str>,
) -> String {
    let chain = cycle
        .iter()
        .map(|token| match module_of(token) {
            Some(module) => format!("{token} (in module {module})"),
            None => token.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n    -> ");
    format!(
        "Circular dependency detected between providers:\n    {chain}\n\
         A provider cannot be built before a provider it depends on. Break the cycle: extract the \
         shared logic into a third provider both depend on, or inject `ModuleRef` into one side and \
         resolve the other lazily at call time."
    )
}

#[cfg(test)]
mod tests {
    use super::find_dependency_cycle;
    use rustc_hash::FxHashMap;

    fn graph(edges: &[(&str, &[&str])]) -> FxHashMap<String, Vec<String>> {
        let mut g: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for (node, deps) in edges {
            g.entry(node.to_string())
                .or_default()
                .extend(deps.iter().map(|d| d.to_string()));
        }
        g
    }

    #[test]
    fn detects_two_provider_cycle() {
        let g = graph(&[("ServiceA", &["ServiceB"]), ("ServiceB", &["ServiceA"])]);
        let cycle = find_dependency_cycle(&g).expect("cycle should be found");
        assert_eq!(cycle.first(), cycle.last());
        assert!(cycle.contains(&"ServiceA".to_string()));
        assert!(cycle.contains(&"ServiceB".to_string()));
    }

    #[test]
    fn detects_self_cycle() {
        let g = graph(&[("A", &["A"])]);
        assert_eq!(
            find_dependency_cycle(&g),
            Some(vec!["A".into(), "A".into()])
        );
    }

    #[test]
    fn detects_longer_cycle() {
        let g = graph(&[("A", &["B"]), ("B", &["C"]), ("C", &["A"])]);
        let cycle = find_dependency_cycle(&g).expect("cycle should be found");
        assert_eq!(cycle.first(), cycle.last());
        for t in ["A", "B", "C"] {
            assert!(cycle.contains(&t.to_string()));
        }
    }

    #[test]
    fn acyclic_graph_has_no_cycle() {
        let g = graph(&[("A", &["B", "D"]), ("B", &["C"]), ("C", &[]), ("D", &[])]);
        assert_eq!(find_dependency_cycle(&g), None);
    }

    #[test]
    fn dependency_on_non_provider_token_is_not_a_cycle() {
        // "MISSING" is only ever a dependency, never a provider key.
        let g = graph(&[("A", &["MISSING"])]);
        assert_eq!(find_dependency_cycle(&g), None);
    }

    #[test]
    fn diamond_without_cycle_is_acyclic() {
        // A depends on B and C; both depend on D. No back-edge.
        let g = graph(&[("A", &["B", "C"]), ("B", &["D"]), ("C", &["D"]), ("D", &[])]);
        assert_eq!(find_dependency_cycle(&g), None);
    }
}
