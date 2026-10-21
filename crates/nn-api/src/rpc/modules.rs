//! ModuleService: helper containers next to servers.

use std::collections::HashMap;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_orchestrator::modules::{Lifecycle, ModuleState, template};
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::module_sites;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::servers::node_error;
use crate::rpc::{audit, require, store_error};

pub(crate) struct ModuleRpc {
    state: AppState,
}

impl ModuleRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn server(&self, id: &str) -> Result<nn_store::Server, ConnectError> {
        self.state
            .auth
            .store()
            .find_server(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("server not found"))
    }

    async fn module(&self, id: &str) -> Result<(nn_store::Module, nn_node::Node), ConnectError> {
        let module = self
            .state
            .auth
            .store()
            .find_module(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("no such module"))?;
        let node = self.state.nodes.for_server_id(&module.server_id).await;
        Ok((module, node))
    }

    /// The hostnames a module's own address makes.
    async fn site_addresses(&self, module: &nn_store::Module) -> Vec<String> {
        let Some(host) = module.web_host.as_deref() else {
            return Vec::new();
        };
        let store = self.state.auth.store();
        let (Ok(Some(server)), Ok(routes)) = (
            store.find_server(&module.server_id).await,
            store.list_routes().await,
        ) else {
            return Vec::new();
        };
        let zones = crate::cloudflare::zones(&self.state).await;
        module_sites::addresses(
            host,
            &module_sites::server_addresses(&server, &routes),
            &zones,
        )
    }

    /// Adds a module NetherNode serves itself (the status page): just its
    /// record, since there's no container or folder. Works for servers on
    /// any machine.
    async fn add_panel_module(
        &self,
        server: &nn_store::Server,
        t: &template::Template,
        name: Option<&str>,
        settings: &Map<String, Value>,
    ) -> Result<nn_store::Module, ConnectError> {
        let name = name.unwrap_or(&t.id);
        if !template::valid_name(name) {
            return Err(ConnectError::invalid_argument(
                "Module names are up to 32 lowercase letters, digits and dashes.",
            ));
        }
        let settings = template::settings(t, settings).map_err(ConnectError::invalid_argument)?;
        let store = self.state.auth.store();
        if store
            .list_modules(Some(&server.id))
            .await
            .map_err(store_error)?
            .iter()
            .any(|m| m.name == name)
        {
            return Err(ConnectError::already_exists(format!(
                "{} already has a module called {name}.",
                server.name
            )));
        }
        store
            .insert_module(nn_store::NewModule {
                server_id: &server.id,
                template: &t.id,
                name,
                settings_json: &Value::Object(settings).to_string(),
                lifecycle: t.lifecycle.as_str(),
                memory_mb: t.memory_mb,
                web_port: None,
                web_host: t.web.as_ref().and_then(|w| w.prefix.as_deref()),
            })
            .await
            .map_err(store_error)
    }

    async fn to_proto(
        &self,
        node: &nn_node::Node,
        module: nn_store::Module,
        state: Option<ModuleState>,
    ) -> pb::Module {
        // One NetherNode serves itself is always up.
        let state = if template::is_panel(&module.template) {
            ModuleState {
                status: "running".into(),
                message: None,
            }
        } else {
            state.unwrap_or(ModuleState {
                status: "absent".into(),
                message: None,
            })
        };
        let site_addresses = self.site_addresses(&module).await;
        pb::Module {
            site_addresses,
            web_host: module.web_host.clone().unwrap_or_default(),
            web_public: module.web_public,
            web_address: module
                .web_port
                .and_then(|p| node.module_address(p))
                .unwrap_or_default(),
            id: module.id,
            server_id: module.server_id,
            template: module.template,
            name: module.name,
            settings_json: module.settings_json,
            lifecycle: module.lifecycle,
            memory_mb: u32::try_from(module.memory_mb).unwrap_or(0),
            status: state.status,
            message: state.message.unwrap_or_default(),
            ..Default::default()
        }
    }
}

fn settings(json: &str) -> Result<Map<String, Value>, ConnectError> {
    if json.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(json) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(ConnectError::invalid_argument(
            "settings must be a JSON object",
        )),
    }
}

fn lifecycle(s: &str) -> Result<Option<Lifecycle>, ConnectError> {
    if s.is_empty() {
        return Ok(None);
    }
    Lifecycle::parse(s).map(Some).ok_or_else(|| {
        ConnectError::invalid_argument("lifecycle is with-server, always, or manual")
    })
}

fn proto_template(t: &template::Template) -> pb::ModuleTemplate {
    pb::ModuleTemplate {
        id: t.id.clone(),
        name: t.name.clone(),
        description: t.description.clone(),
        version: t.version.clone(),
        image: t.image.clone(),
        lifecycle: t.lifecycle.as_str().to_owned(),
        memory_mb: u32::try_from(t.memory_mb).unwrap_or(0),
        has_web: t.web.is_some(),
        panel: t.panel,
        notes: t.notes.clone(),
        settings: t
            .settings
            .iter()
            .map(|s| pb::ModuleSetting {
                key: s.key.clone(),
                label: s.label.clone(),
                help: s.help.clone(),
                kind: serde_json::to_value(s.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                default_json: s.default.to_string(),
                required: s.required,
                min: s.min,
                max: s.max,
                choices: s.choices.clone(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[allow(refining_impl_trait)]
impl pb::ModuleService for ModuleRpc {
    async fn list_module_templates(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListModuleTemplatesRequest>,
    ) -> ServiceResult<pb::ListModuleTemplatesResponse> {
        require(&ctx, Permission::ManageServers)?;
        Response::ok(pb::ListModuleTemplatesResponse {
            templates: template::built_in().iter().map(proto_template).collect(),
            ..Default::default()
        })
    }

    async fn list_modules(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListModulesRequest>,
    ) -> ServiceResult<pb::ListModulesResponse> {
        require(&ctx, Permission::ManageServers)?;
        let server = self.server(request.server_id).await?;
        let node = self.state.nodes.for_server(&server);
        let recorded = self
            .state
            .auth
            .store()
            .list_modules(Some(&server.id))
            .await
            .map_err(store_error)?;
        // What the containers are doing; without it, still list what's
        // recorded.
        let (mut states, unavailable) = match node.modules(&server.id).await {
            Ok(found) => (
                found
                    .into_iter()
                    .map(|(m, s)| (m.id, s))
                    .collect::<HashMap<_, _>>(),
                String::new(),
            ),
            Err(e) => (HashMap::new(), e.to_string()),
        };
        let mut modules = Vec::new();
        for m in recorded {
            let state = states.remove(&m.id);
            modules.push(self.to_proto(&node, m, state).await);
        }
        Response::ok(pb::ListModulesResponse {
            modules,
            unavailable,
            ..Default::default()
        })
    }

    async fn add_module(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::AddModuleRequest>,
    ) -> ServiceResult<pb::AddModuleResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let server = self.server(request.server_id).await?;
        crate::moves::not_moving(&self.state, &server.id)?;
        let node = self.state.nodes.for_server(&server);
        let name = Some(request.name.trim()).filter(|n| !n.is_empty());
        let memory = (request.memory_mb > 0).then(|| i64::from(request.memory_mb));
        let module = match template::find(request.template).filter(|t| t.panel) {
            Some(t) => {
                self.add_panel_module(&server, t, name, &settings(request.settings_json)?)
                    .await?
            }
            None => node
                .add_module(
                    &server.id,
                    request.template,
                    name,
                    &settings(request.settings_json)?,
                    lifecycle(request.lifecycle)?,
                    memory,
                )
                .await
                .map_err(node_error)?,
        };
        audit(
            &self.state,
            &user,
            "module.add",
            ("server", &server.id),
            json!({ "module": module.id, "template": module.template, "name": module.name }),
        )
        .await;
        // Start it if it should be running now; failures show as a toast.
        if !template::is_panel(&module.template)
            && let Some(lifecycle) = Lifecycle::parse(&module.lifecycle)
            && nn_orchestrator::modules::wanted(lifecycle, &server.status) == Some(true)
        {
            let (state, node, id, server_id, name) = (
                self.state.clone(),
                node.clone(),
                module.id.clone(),
                server.id.clone(),
                module.name.clone(),
            );
            tokio::spawn(async move {
                if let Err(e) = node.start_module(&id).await {
                    state
                        .events
                        .publish(nn_events::Event::ServerOperationFailed {
                            server_id,
                            operation: format!("start the {name} module"),
                            message: e.to_string(),
                        });
                }
            });
        }
        Response::ok(pb::AddModuleResponse {
            module: self.to_proto(&node, module, None).await.into(),
            ..Default::default()
        })
    }

    async fn update_module(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateModuleRequest>,
    ) -> ServiceResult<pb::UpdateModuleResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (module, node) = self.module(request.id).await?;
        let updated = if let Some(t) = template::find(&module.template).filter(|t| t.panel) {
            let settings = template::settings(t, &settings(request.settings_json)?)
                .map_err(ConnectError::invalid_argument)?;
            let store = self.state.auth.store();
            store
                .update_module(
                    &module.id,
                    &Value::Object(settings).to_string(),
                    &module.lifecycle,
                    module.memory_mb,
                )
                .await
                .map_err(store_error)?;
            store
                .find_module(&module.id)
                .await
                .map_err(store_error)?
                .ok_or_else(|| ConnectError::not_found("no such module"))?
        } else {
            let lifecycle = lifecycle(request.lifecycle)?
                .ok_or_else(|| ConnectError::invalid_argument("choose when it runs"))?;
            node.update_module(
                &module.id,
                &settings(request.settings_json)?,
                lifecycle,
                i64::from(request.memory_mb),
            )
            .await
            .map_err(node_error)?
        };
        audit(
            &self.state,
            &user,
            "module.update",
            ("server", &module.server_id),
            json!({ "module": module.id, "name": module.name }),
        )
        .await;
        Response::ok(pb::UpdateModuleResponse {
            module: self.to_proto(&node, updated, None).await.into(),
            ..Default::default()
        })
    }

    async fn start_module(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::StartModuleRequest>,
    ) -> ServiceResult<pb::StartModuleResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (module, node) = self.module(request.id).await?;
        crate::moves::not_moving(&self.state, &module.server_id)?;
        node.start_module(&module.id).await.map_err(node_error)?;
        audit(
            &self.state,
            &user,
            "module.start",
            ("server", &module.server_id),
            json!({ "module": module.id, "name": module.name }),
        )
        .await;
        Response::ok(pb::StartModuleResponse::default())
    }

    async fn stop_module(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::StopModuleRequest>,
    ) -> ServiceResult<pb::StopModuleResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (module, node) = self.module(request.id).await?;
        node.stop_module(&module.id).await.map_err(node_error)?;
        audit(
            &self.state,
            &user,
            "module.stop",
            ("server", &module.server_id),
            json!({ "module": module.id, "name": module.name }),
        )
        .await;
        Response::ok(pb::StopModuleResponse::default())
    }

    async fn remove_module(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RemoveModuleRequest>,
    ) -> ServiceResult<pb::RemoveModuleResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (module, node) = self.module(request.id).await?;
        if template::is_panel(&module.template) {
            self.state
                .auth
                .store()
                .delete_module(&module.id)
                .await
                .map_err(store_error)?;
        } else {
            node.remove_module(&module.id, request.delete_files)
                .await
                .map_err(node_error)?;
        }
        module_sites::refresh(&self.state).await;
        audit(
            &self.state,
            &user,
            "module.remove",
            ("server", &module.server_id),
            json!({ "module": module.id, "name": module.name, "delete_files": request.delete_files }),
        )
        .await;
        Response::ok(pb::RemoveModuleResponse::default())
    }

    async fn set_module_site(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetModuleSiteRequest>,
    ) -> ServiceResult<pb::SetModuleSiteResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (module, node) = self.module(request.id).await?;
        if !module_sites::has_page(&module) {
            return Err(ConnectError::failed_precondition(
                "This module has no web page.",
            ));
        }
        let host =
            module_sites::check_host(request.web_host).map_err(ConnectError::invalid_argument)?;
        let store = self.state.auth.store();
        // Each hostname can lead to one module.
        let mut mine = module.clone();
        mine.web_host = Some(host.clone());
        let wanted = self.site_addresses(&mine).await;
        for other in store.list_modules(None).await.map_err(store_error)? {
            if other.id == module.id || !other.web_public {
                continue;
            }
            let theirs = self.site_addresses(&other).await;
            if let Some(taken) = wanted.iter().find(|a| theirs.contains(a)) {
                return Err(ConnectError::already_exists(format!(
                    "{taken} is already another module's address."
                )));
            }
        }
        store
            .set_module_site(&module.id, Some(&host), request.web_public)
            .await
            .map_err(store_error)?;
        module_sites::refresh(&self.state).await;
        audit(
            &self.state,
            &user,
            "module.site",
            ("server", &module.server_id),
            json!({ "module": module.id, "name": module.name, "web_host": host, "public": request.web_public }),
        )
        .await;
        let saved = store
            .find_module(&module.id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("no such module"))?;
        Response::ok(pb::SetModuleSiteResponse {
            module: self.to_proto(&node, saved, None).await.into(),
            ..Default::default()
        })
    }

    async fn get_module_logs(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetModuleLogsRequest>,
    ) -> ServiceResult<pb::GetModuleLogsResponse> {
        require(&ctx, Permission::ManageServers)?;
        let (module, node) = self.module(request.id).await?;
        let lines = if request.lines == 0 {
            200
        } else {
            request.lines as usize
        };
        let lines = node
            .module_logs(&module.id, lines)
            .await
            .map_err(node_error)?;
        Response::ok(pb::GetModuleLogsResponse {
            lines,
            ..Default::default()
        })
    }
}
