//! Picks the project an exported span belongs to when one receiver serves several graphs.

use hoshizora_model::RuntimeSpan;

/// One candidate project: its id, the `service.name`s it owns and whether it can place the span.
pub trait ProjectCandidate {
    fn id(&self) -> &str;
    fn services(&self) -> &[String];
    fn locates(&self, span: &RuntimeSpan) -> bool;
}

/// Index of the project `span` goes to: the one owning its `service.name`, else `hint`
/// (from the `x-hoshizora-project` header or `?project=`), else the one its code location
/// resolves in, else the first project. `None` only when there are no projects or the hint
/// names an unknown project.
pub fn project_for_span<P: ProjectCandidate>(
    span: &RuntimeSpan,
    hint: Option<&str>,
    projects: &[P],
) -> Result<usize, RouteError> {
    if projects.is_empty() {
        return Err(RouteError::NoProjects);
    }
    if let Some(i) = projects
        .iter()
        .position(|p| p.services().iter().any(|s| s == &span.service))
    {
        return Ok(i);
    }
    if let Some(hint) = hint.map(str::trim).filter(|h| !h.is_empty()) {
        return projects
            .iter()
            .position(|p| p.id() == hint)
            .ok_or_else(|| RouteError::UnknownProject(hint.to_string()));
    }
    Ok(projects.iter().position(|p| p.locates(span)).unwrap_or(0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    NoProjects,
    UnknownProject(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProjects => write!(f, "no project registered"),
            Self::UnknownProject(p) => write!(f, "unknown project {p}"),
        }
    }
}

impl std::error::Error for RouteError {}

#[cfg(test)]
mod tests {
    use hoshizora_model::SpanKind;

    use super::*;

    struct P(&'static str, Vec<String>, bool);

    impl ProjectCandidate for P {
        fn id(&self) -> &str {
            self.0
        }
        fn services(&self) -> &[String] {
            &self.1
        }
        fn locates(&self, _span: &RuntimeSpan) -> bool {
            self.2
        }
    }

    fn span(service: &str) -> RuntimeSpan {
        RuntimeSpan {
            trace_id: "t".into(),
            span_id: "s".into(),
            parent_span_id: None,
            service: service.into(),
            name: "x".into(),
            kind: SpanKind::Internal,
            start_unix_nanos: 0,
            end_unix_nanos: 1,
            error: false,
            attrs: Default::default(),
            node: None,
            join: None,
        }
    }

    #[test]
    fn service_then_hint_then_location_then_first() {
        let projects = vec![
            P("a", vec![], false),
            P("b", vec!["api".into()], false),
            P("c", vec![], true),
        ];
        assert_eq!(project_for_span(&span("api"), Some("a"), &projects), Ok(1));
        assert_eq!(project_for_span(&span("web"), Some("a"), &projects), Ok(0));
        assert_eq!(
            project_for_span(&span("web"), Some("zz"), &projects),
            Err(RouteError::UnknownProject("zz".into()))
        );
        assert_eq!(project_for_span(&span("web"), None, &projects), Ok(2));
        assert_eq!(project_for_span(&span("web"), Some(" "), &projects), Ok(2));
        let none = vec![P("a", vec![], false)];
        assert_eq!(project_for_span(&span("web"), None, &none), Ok(0));
        assert_eq!(
            project_for_span(&span("web"), None, &Vec::<P>::new()),
            Err(RouteError::NoProjects)
        );
    }
}
