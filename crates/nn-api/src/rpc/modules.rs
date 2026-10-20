//! ModuleService: helper containers next to servers.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_orchestrator::modules::{Lifecycle, ModuleState, template};
use serde_json::{Map, Value, json};

use crate::AppState;
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

    fn to_proto(
        &self,
        node: &nn_node::Node,
        module: nn_store::Module,
        state: Option<ModuleState>,
    ) -> pb::Module {
        let state = state.unwrap_or(ModuleState {
            status: "absent".into(),
            message: None,
        });
        pb::Module {
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
        let (modules, unavailable) = match node.modules(&server.id).await {
            Ok(found) => (
                found
                    .into_iter()
                    .map(|(m, s)| self.to_proto(&node, m, Some(s)))
                    .collect(),
                String::new(),
            ),
            // Still list what's recorded, without what it's doing.
            Err(e) => (
                self.state
                    .auth
                    .store()
                    .list_modules(Some(&server.id))
                    .await
                    .map_err(store_error)?
                    .into_iter()
                    .map(|m| self.to_proto(&node, m, None))
                    .collect(),
                e.to_string(),
            ),
        };
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
        let module = node
            .add_module(
                &server.id,
                request.template,
                name,
                &settings(request.settings_json)?,
                lifecycle(request.lifecycle)?,
                memory,
            )
            .await
            .map_err(node_error)?;
        audit(
            &self.state,
            &user,
            "module.add",
            ("server", &server.id),
            json!({ "module": module.id, "template": module.template, "name": module.name }),
        )
        .await;
        // Start it if it should be running now; failures show as a toast.
        if let Some(lifecycle) = Lifecycle::parse(&module.lifecycle)
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
            module: self.to_proto(&node, module, None).into(),
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
        let lifecycle = lifecycle(request.lifecycle)?
            .ok_or_else(|| ConnectError::invalid_argument("choose when it runs"))?;
        let updated = node
            .update_module(
                &module.id,
                &settings(request.settings_json)?,
                lifecycle,
                i64::from(request.memory_mb),
            )
            .await
            .map_err(node_error)?;
        audit(
            &self.state,
            &user,
            "module.update",
            ("server", &module.server_id),
            json!({ "module": module.id, "name": module.name }),
        )
        .await;
        Response::ok(pb::UpdateModuleResponse {
            module: self.to_proto(&node, updated, None).into(),
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
        node.remove_module(&module.id, request.delete_files)
            .await
            .map_err(node_error)?;
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
