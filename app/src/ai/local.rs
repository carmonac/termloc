//! Runs agent turns on the user's Ollama instance and tracks the models it offers.
use std::sync::{Arc, LazyLock};

use anyhow::anyhow;
use futures::future::BoxFuture;
use futures::{FutureExt as _, Stream};
use local_agent::ollama::{ChatRequest, ChatStream};
use local_agent::oneshot::{OneShot, QuickModel};
use local_agent::{ChatBackend, LocalAgentConfig, LocalModel, OllamaClient};
use parking_lot::RwLock;
use settings::Setting as _;
use url::Url;
use warp_core::channel::ChannelState;
use warp_multi_agent_api::{Request, ResponseEvent};
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::ai::llms::LLMPreferences;
use crate::settings::{AISettings, AISettingsChangedEvent};

/// What agent turns read from outside the app's entity system.
#[derive(Clone, Default)]
struct Runtime {
    url: String,
    context_window: u32,
    /// Every installed model, including those the agent can't use.
    models: Vec<LocalModel>,
    /// The model chosen for quick requests. Empty means the agent's default.
    quick_model: String,
}

impl Runtime {
    fn quick_model(&self) -> Option<QuickModel> {
        let model = self
            .models
            .iter()
            .find(|model| model.name == self.quick_model)
            .or_else(|| {
                self.models
                    .iter()
                    .filter(|model| model.supports_tools)
                    .max_by_key(|model| model.size)
            })?;
        Some(QuickModel {
            name: model.name.clone(),
            thinks: model.supports_thinking,
            num_ctx: context_window(self.context_window, model),
        })
    }
}

static RUNTIME: LazyLock<RwLock<Runtime>> = LazyLock::new(Default::default);

pub(crate) fn is_enabled() -> bool {
    ChannelState::is_local_only()
}

/// The models that can drive the agent, which works entirely through tool calls. Largest first,
/// because the first is the default and small models tend to be poor at tool calls.
fn agent_models(mut models: Vec<LocalModel>) -> Vec<LocalModel> {
    models.retain(|model| model.supports_tools);
    models.sort_by_key(|model| std::cmp::Reverse(model.size));
    models
}

fn unavailable_message(url: &str) -> String {
    format!(
        "No Ollama model is available at {url}. Start Ollama and install a model with tool \
         support, for example `ollama pull qwen3:8b`."
    )
}

/// Reports why a turn can't run through the same path as a failed model call.
struct UnavailableBackend(String);

impl ChatBackend for UnavailableBackend {
    fn chat(&self, _request: ChatRequest) -> BoxFuture<'static, anyhow::Result<ChatStream>> {
        let message = self.0.clone();
        async move { Err(anyhow!(message)) }.boxed()
    }
}

/// Picks the model a request asked for, or the first one when it named something else, like a
/// selection made before that model was removed.
fn select_model<'a>(models: &'a [LocalModel], requested: &str) -> Option<&'a LocalModel> {
    models
        .iter()
        .find(|model| model.name == requested)
        .or(models.first())
}

fn context_window(limit: u32, model: &LocalModel) -> u32 {
    model
        .context_length
        .map_or(limit, |length| length.min(limit))
}

/// Reads the runtime state. Ollama may have started after the app did, so an empty model list is
/// checked again first.
async fn current_runtime() -> Runtime {
    let mut runtime = RUNTIME.read().clone();
    if runtime.models.is_empty()
        && let Ok(models) = OllamaClient::new(runtime.url.clone()).list_models().await
    {
        runtime.models = models;
        RUNTIME.write().models = runtime.models.clone();
    }
    runtime
}

/// The client for single-request features, when a model is available.
pub(crate) async fn one_shot() -> anyhow::Result<OneShot> {
    let runtime = current_runtime().await;
    let model = runtime
        .quick_model()
        .ok_or_else(|| anyhow!(unavailable_message(&runtime.url)))?;
    Ok(OneShot::new(
        Arc::new(OllamaClient::new(runtime.url)),
        model,
    ))
}

/// Starts a local turn.
pub(crate) async fn run_turn(request: Request) -> impl Stream<Item = ResponseEvent> + Send {
    let runtime = current_runtime().await;
    let models = agent_models(runtime.models.clone());

    let requested = request
        .settings
        .as_ref()
        .and_then(|settings| settings.model_config.as_ref())
        .map(|config| config.base.as_str())
        .unwrap_or_default();
    let (backend, config): (Arc<dyn ChatBackend>, LocalAgentConfig) =
        match select_model(&models, requested) {
            Some(model) => (
                Arc::new(OllamaClient::new(runtime.url.clone())),
                LocalAgentConfig {
                    model: model.name.clone(),
                    num_ctx: context_window(runtime.context_window, model),
                    title_model: runtime.quick_model(),
                },
            ),
            None => (
                Arc::new(UnavailableBackend(unavailable_message(&runtime.url))),
                LocalAgentConfig {
                    model: String::new(),
                    num_ctx: runtime.context_window,
                    title_model: None,
                },
            ),
        };
    local_agent::run_turn(backend, config, request)
}

#[derive(Clone, Debug, PartialEq)]
pub enum LocalModelsState {
    Loading,
    Ready,
    Unavailable(String),
}

pub enum LocalModelsEvent {
    Changed,
}

/// The models installed in Ollama, kept in sync with the user's settings.
pub struct LocalModels {
    state: LocalModelsState,
    /// The models that can run the agent, best first.
    models: Vec<LocalModel>,
    installed_models: Vec<LocalModel>,
}

impl LocalModels {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, _, event, ctx| {
            if matches!(
                event,
                AISettingsChangedEvent::OllamaUrl { .. }
                    | AISettingsChangedEvent::LocalContextWindow { .. }
                    | AISettingsChangedEvent::LocalQuickModel { .. }
            ) {
                me.refresh(ctx);
            }
        });
        let mut me = Self {
            state: LocalModelsState::Loading,
            models: Vec::new(),
            installed_models: Vec::new(),
        };
        me.refresh(ctx);
        me
    }

    pub fn state(&self) -> &LocalModelsState {
        &self.state
    }

    pub fn models(&self) -> &[LocalModel] {
        &self.models
    }

    pub fn installed_models(&self) -> &[LocalModel] {
        &self.installed_models
    }

    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        let settings = AISettings::as_ref(ctx);
        let url = settings.ollama_url.value().clone();
        let context_window = *settings.local_context_window.value();
        let quick_model = settings.local_quick_model.value().clone();
        allow_ollama_host(&url);
        {
            let mut runtime = RUNTIME.write();
            runtime.url = url.clone();
            runtime.context_window = context_window;
            runtime.quick_model = quick_model;
        }

        self.state = LocalModelsState::Loading;
        ctx.emit(LocalModelsEvent::Changed);
        let _ = ctx.spawn(
            async move {
                OllamaClient::new(url.clone())
                    .list_models()
                    .await
                    .map_err(|_| unavailable_message(&url))
            },
            Self::on_models_fetched,
        );
    }

    fn on_models_fetched(
        &mut self,
        result: Result<Vec<LocalModel>, String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let url = RUNTIME.read().url.clone();
        (self.installed_models, self.state) = match result {
            Ok(models) if agent_models(models.clone()).is_empty() => (
                models,
                LocalModelsState::Unavailable(unavailable_message(&url)),
            ),
            Ok(models) => (models, LocalModelsState::Ready),
            Err(message) => (Vec::new(), LocalModelsState::Unavailable(message)),
        };
        RUNTIME.write().models = self.installed_models.clone();
        self.models = agent_models(self.installed_models.clone());
        if !self.models.is_empty() {
            let models = self.models.clone();
            LLMPreferences::handle(ctx).update(ctx, |preferences, ctx| {
                preferences.set_local_models(&models, ctx);
            });
        }
        ctx.emit(LocalModelsEvent::Changed);
    }
}

/// The user chose this server, so it counts as allowed even on another machine.
fn allow_ollama_host(url: &str) {
    if let Some(host) = Url::parse(url).ok().as_ref().and_then(Url::host_str) {
        egress::allow_host(host);
    }
}

impl Entity for LocalModels {
    type Event = LocalModelsEvent;
}

impl SingletonEntity for LocalModels {}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
