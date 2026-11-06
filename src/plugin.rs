use pulldown_cmark::Event;

#[derive(Default, Clone)]
pub struct TransformContext<'a> {
    pub current_url: Option<&'a str>,
    pub link_resolver: Option<&'a (dyn Fn(&str, &str) -> (String, Option<String>) + Send + Sync)>,
    pub media_resolver: Option<&'a (dyn Fn(&str) -> Option<String> + Send + Sync)>,
}

pub trait MarkdownPlugin: Send + Sync {
    fn name(&self) -> &'static str;

    /// Returns the character that triggers this plugin (e.g. '[' or '>')
    /// Used by the hot loop to bypass unnecessary checks.
    fn trigger_byte(&self) -> Option<u8> {
        None
    }

    /// Process or rewrite an event stream segment
    fn transform<'a>(&self, event: Event<'a>) -> Option<Vec<Event<'a>>> {
        let _ = event;
        None
    }

    /// Context-aware transform (defaults to `transform`)
    fn transform_with_context<'a>(
        &self,
        event: Event<'a>,
        _ctx: &TransformContext<'a>,
    ) -> Option<Vec<Event<'a>>> {
        self.transform(event)
    }
}

pub struct PluginRegistry {
    plugins: Vec<Box<dyn MarkdownPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    pub fn register(&mut self, plugin: Box<dyn MarkdownPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn plugins(&self) -> &[Box<dyn MarkdownPlugin>] {
        &self.plugins
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
