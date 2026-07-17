use super::ToniContainer;
use anyhow::{Result, anyhow};
use rustc_hash::FxHashMap;
use std::{cell::RefCell, rc::Rc};

pub struct DependencyGraph {
    container: Rc<RefCell<ToniContainer>>,
    module_token: String,
    visited: FxHashMap<String, bool>,
    temp_mark: FxHashMap<String, bool>,
    ordered: Vec<String>,
}

impl DependencyGraph {
    pub fn new(container: Rc<RefCell<ToniContainer>>, module_token: String) -> Self {
        Self {
            container,
            module_token,
            visited: FxHashMap::default(),
            temp_mark: FxHashMap::default(),
            ordered: Vec::new(),
        }
    }

    pub fn get_ordered_providers_token(mut self) -> Result<Vec<String>> {
        let (providers, multi_providers) = {
            let container = self.container.borrow();
            let providers_map = container.get_providers_factory(&self.module_token)?;
            let providers = providers_map
                .iter()
                .map(|(token, provider)| (token.clone(), provider.get_dependencies()))
                .collect::<Vec<(String, Vec<String>)>>();
            // Map from multi-collection base token (e.g. "PLUGINS") to the contributing
            // provider tokens within this module so the topological sort can treat all
            // contributors as implicit dependencies of any provider that injects the base token.
            let multi_providers: FxHashMap<String, Vec<String>> = container
                .get_multi_providers()
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
        providers: &Vec<(String, Vec<String>)>,
        multi_providers: &FxHashMap<String, Vec<String>>,
    ) -> Result<()> {
        if self.temp_mark.contains_key(&token) {
            return Err(anyhow!(
                "Circular dependency detected for provider: {}",
                token
            ));
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
}

/// Find one dependency cycle in a provider graph, if any exists.
///
/// `adjacency` maps each provider token to the tokens it depends on. Only tokens that
/// are themselves keys are treated as providers; a dependency on a value/factory token
/// or a token no module produces is a leaf and cannot be part of a cycle.
///
/// Returns the cycle as `[A, B, …, A]` — the entry token repeated at the end — or `None`
/// when the graph is acyclic. Roots are visited in sorted order so the result is
/// deterministic for a given graph. Used only on the failure path, where the per-module
/// [`DependencyGraph`] sort has already excluded within-module cycles, to name a cycle
/// that spans modules.
pub fn find_dependency_cycle(adjacency: &FxHashMap<String, Vec<String>>) -> Option<Vec<String>> {
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
