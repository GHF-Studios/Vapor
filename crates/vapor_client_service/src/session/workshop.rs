//! Shared-service orchestration for Workshop discovery and publication.

use super::composition::apply_operation_result_to_composition;
use super::*;

impl ClientSession {
    fn validate_workshop_stage(&mut self, selector: &str) -> Result<PathBuf, ServiceError> {
        let Some(workspace_root) = self.bootstrap.workspace_root.clone() else {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "Open the authored content workspace before staging a Workshop item."
                    .to_owned(),
                retryable: false,
            });
        };
        self.refresh_catalog();
        if let Some(error) = self.catalog.status().error.as_ref() {
            return Err(ServiceError {
                code: ServiceErrorCode::ProviderFailure,
                message: format!(
                    "Vapor Client could not refresh on-device content before staging: {error}"
                ),
                retryable: true,
            });
        }
        if self.catalog.status().omitted > 0 {
            return Err(ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message: "The on-device catalog is incomplete; content conflicts cannot be confirmed before staging.".to_owned(),
                retryable: false,
            });
        }
        if !self.workspace_content.source_projects_loaded
            || self.workspace_content.source_projects_error.is_some()
        {
            return Err(ServiceError {
                code: ServiceErrorCode::ProviderFailure,
                message: self
                    .workspace_content
                    .source_projects_error
                    .clone()
                    .unwrap_or_else(|| {
                        "Vapor Client could not refresh the authored project list.".into()
                    }),
                retryable: true,
            });
        }
        let (id, version) = selector
            .split_once('@')
            .map(|(id, version)| (id, Some(version)))
            .unwrap_or((selector, None));
        if id.is_empty()
            || version.is_some_and(str::is_empty)
            || !self
                .workspace_content
                .source_projects
                .iter()
                .any(|project| project.id == id && project.version.as_deref() == version)
        {
            return Err(invalid_client_action_request(
                "Select an exact content identity from the active workspace before staging it.",
            ));
        }
        if self.catalog.entries().iter().any(|entry| {
            entry.id == id && entry.version.as_deref() == version && !entry.conflicts.is_empty()
        }) {
            return Err(invalid_client_action_request(
                "On-device copies of this content version disagree. Resolve the conflict before staging it.",
            ));
        }
        Ok(workspace_root)
    }

    pub(super) fn start_workshop_operation(
        &mut self,
        action: WorkshopAction,
    ) -> Result<(), ServiceError> {
        self.reject_if_operation_in_progress()?;
        let kind = match action {
            WorkshopAction::Search { query, page } => {
                if query.len() > 256 {
                    return Err(invalid_client_action_request(
                        "Workshop search terms must be 256 bytes or fewer.",
                    ));
                }
                if !(1..=1_000).contains(&page) {
                    return Err(invalid_client_action_request(
                        "Workshop search page must be between 1 and 1000.",
                    ));
                }
                WorkshopOperationKind::Search {
                    query: query.trim().to_owned(),
                    page,
                }
            }
            WorkshopAction::LoadSubscriptions => WorkshopOperationKind::LoadSubscriptions,
            WorkshopAction::Subscribe { item_ids } => WorkshopOperationKind::ChangeSubscriptions {
                ids: parse_workshop_ids(item_ids)?,
                subscribe: true,
            },
            WorkshopAction::Unsubscribe { item_ids } => {
                WorkshopOperationKind::ChangeSubscriptions {
                    ids: parse_workshop_ids(item_ids)?,
                    subscribe: false,
                }
            }
            WorkshopAction::Acquire { item_ids } => WorkshopOperationKind::Acquire {
                ids: parse_workshop_ids(item_ids)?
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
            },
            WorkshopAction::Stage {
                selector,
                target_triples,
            } => {
                let workspace_root = self.validate_workshop_stage(&selector)?;
                super::validate_runtime_target_matrix(&target_triples)?;
                self.workshop_review = None;
                self.workshop_batch_review = None;
                WorkshopOperationKind::Stage {
                    selector,
                    target_triples,
                    workspace_root,
                }
            }
            WorkshopAction::StageBatch {
                selectors,
                target_triples,
            } => {
                if selectors.is_empty()
                    || selectors.len() > vapor_client_application::MAX_WORKSHOP_REVIEW_BATCH
                {
                    return Err(invalid_client_action_request(&format!(
                        "Workshop review batches must contain 1 to {} items.",
                        vapor_client_application::MAX_WORKSHOP_REVIEW_BATCH
                    )));
                }
                let mut unique = BTreeSet::new();
                for selector in &selectors {
                    if !unique.insert(selector) {
                        return Err(invalid_client_action_request(
                            "Workshop review batches cannot contain duplicate content identities.",
                        ));
                    }
                    self.validate_workshop_stage(selector)?;
                }
                let workspace_root = self
                    .bootstrap
                    .workspace_root
                    .clone()
                    .ok_or_else(|| invalid_client_action_request("Open a workspace first."))?;
                super::validate_runtime_target_matrix(&target_triples)?;
                self.workshop_review = None;
                self.workshop_batch_review = None;
                WorkshopOperationKind::StageBatch {
                    selectors,
                    target_triples,
                    workspace_root,
                }
            }
            WorkshopAction::Publish {
                review_id,
                account,
                confirmation,
                change_note,
            } => {
                let review = self
                    .workshop_review
                    .as_ref()
                    .filter(|review| review.review_id == review_id)
                    .cloned()
                    .ok_or_else(|| {
                        invalid_client_action_request(
                            "This Workshop review is no longer current. Stage the content again.",
                        )
                    })?;
                let workspace_root = self.validate_workshop_stage(&review.selector)?;
                if workspace_root != review.workspace_root {
                    return Err(invalid_client_action_request(
                        "The active workspace changed after this review was staged.",
                    ));
                }
                let account = account.trim().to_owned();
                if account.is_empty() || account.len() > 128 {
                    return Err(invalid_client_action_request(
                        "Enter a publisher account name of 1 to 128 bytes.",
                    ));
                }
                let expected_confirmation = format!("PUBLISH {} TO {account}", review.preview.id);
                if confirmation.trim() != expected_confirmation {
                    return Err(invalid_client_action_request(&format!(
                        "Type {expected_confirmation} exactly to authorize this Workshop upload."
                    )));
                }
                let change_note = change_note.trim();
                if change_note.len() > 500 {
                    return Err(invalid_client_action_request(
                        "Workshop change notes must be 500 bytes or fewer.",
                    ));
                }
                WorkshopOperationKind::Publish {
                    review: Box::new(review),
                    account,
                    change_note: (!change_note.is_empty()).then(|| change_note.to_owned()),
                }
            }
            WorkshopAction::PublishBatch {
                review_id,
                account,
                confirmation,
                change_note,
            } => {
                let review = self
                    .workshop_batch_review
                    .as_ref()
                    .filter(|review| review.review_id == review_id)
                    .cloned()
                    .ok_or_else(|| {
                        invalid_client_action_request(
                            "This Workshop batch review is no longer current. Stage the batch again.",
                        )
                    })?;
                if self.bootstrap.workspace_root.as_deref() != Some(review.workspace_root.as_path())
                {
                    return Err(invalid_client_action_request(
                        "The active workspace changed after this batch review was staged.",
                    ));
                }
                for selector in &review.preview.selectors {
                    self.validate_workshop_stage(selector)?;
                }
                let account = account.trim().to_owned();
                if account.is_empty() || account.len() > 128 {
                    return Err(invalid_client_action_request(
                        "Enter a publisher account name of 1 to 128 bytes.",
                    ));
                }
                let item_label = if review.preview.selectors.len() == 1 {
                    "ITEM"
                } else {
                    "ITEMS"
                };
                let expected_confirmation = format!(
                    "PUBLISH {} {item_label} TO {account}",
                    review.preview.selectors.len()
                );
                if confirmation.trim() != expected_confirmation {
                    return Err(invalid_client_action_request(&format!(
                        "Type {expected_confirmation} exactly to authorize this Workshop batch upload."
                    )));
                }
                let change_note = change_note.trim();
                if change_note.len() > 500 {
                    return Err(invalid_client_action_request(
                        "Workshop change notes must be 500 bytes or fewer.",
                    ));
                }
                WorkshopOperationKind::PublishBatch {
                    review: Box::new(review),
                    account,
                    change_note: (!change_note.is_empty()).then(|| change_note.to_owned()),
                }
            }
            WorkshopAction::DiscardReview => {
                return Err(invalid_client_action_request(
                    "Workshop review discard must use the synchronous service path.",
                ));
            }
        };
        let auto_install = self
            .auto_install_context(&kind)
            .map_err(|message| ServiceError {
                code: ServiceErrorCode::ActionUnavailable,
                message,
                retryable: true,
            })?;
        let action_id = workshop_operation_action_id(&kind);
        let fixture_mode = self.bootstrap.fixture_mode;
        let application_executable = self.bootstrap.application_executable.clone();
        let workshop_app_id = self.bootstrap.workshop_app_id;
        let fixture_subscriptions = self.fixture_workshop_subscriptions.clone();
        let action_for_worker = kind.clone();
        self.spawn_background_operation(
            BackgroundOperationSpec {
                domain: "studio-workshop",
                action: action_id,
                thread_prefix: "studio-workshop",
                failure_label: "Workshop operation",
                composition_id: None,
                phase: OperationPhase::Resolving,
            },
            move |context| {
                execute_workshop_operation(
                    &application_executable,
                    fixture_mode,
                    workshop_app_id,
                    fixture_subscriptions,
                    auto_install,
                    action_for_worker,
                    context,
                )
            },
            workshop_result_outcome,
            move |id, result| {
                BackgroundOperationMessage::CompleteWorkshop(Box::new(
                    WorkshopOperationCompletion {
                        id,
                        action: kind,
                        result,
                    },
                ))
            },
        )
    }

    pub(super) fn auto_install_context(
        &self,
        kind: &WorkshopOperationKind,
    ) -> Result<AutoInstallContext, String> {
        let requested_ids = match kind {
            WorkshopOperationKind::Acquire { ids } => ids.iter().cloned().collect::<BTreeSet<_>>(),
            WorkshopOperationKind::ChangeSubscriptions {
                ids,
                subscribe: true,
            } => ids.iter().map(u64::to_string).collect::<BTreeSet<_>>(),
            _ => return Ok(AutoInstallContext::default()),
        };
        let mut context = AutoInstallContext::default();
        for item in self.catalog.entries().iter().filter(|item| {
            item.published_file_id
                .as_ref()
                .is_some_and(|id| requested_ids.contains(id))
        }) {
            let reference = ContentReference::parse(item.id.clone(), item.version.clone())
                .map_err(|error| format!("invalid cached content identity: {error}"))?;
            let content_id = reference
                .vapor_id()
                .map_err(|error| format!("invalid cached content ID: {error}"))?;
            if !vapor_client_domain::library_auto_install_enabled(
                &self.durable_state.library_policies,
                &content_id,
            ) {
                continue;
            }
            let constraints = vapor_client_domain::library_constraints(
                &self.durable_state.compositions,
                &content_id,
            )
            .map_err(|missing| {
                format!(
                    "Auto-Install paused: composition '{}' has an unresolved version constraint for {}. Resolve that composition before updating this item.",
                    missing.composition_id, content_id
                )
            })?;
            context.constraints.insert(
                content_id.to_string(),
                constraints.iter().map(ToString::to_string).collect(),
            );
            for composition in &self.durable_state.compositions.compositions {
                if composition.root_package.is_some() && composition.resolved_graph.is_none() {
                    return Err(format!(
                        "Auto-Install paused: composition '{}' has not been resolved, so Vapor cannot verify that this update is safe.",
                        composition.name
                    ));
                }
                let depends_on_item = composition.resolved_graph.as_ref().is_some_and(|graph| {
                    graph
                        .nodes
                        .iter()
                        .any(|node| node.content.id == content_id.as_str())
                });
                if depends_on_item
                    && composition.root_package.is_some()
                    && matches!(
                        composition.installation,
                        InstallationState::Installed { .. } | InstallationState::OutOfDate { .. }
                    )
                {
                    context
                        .compositions
                        .entry(content_id.to_string())
                        .or_default()
                        .push(AutoInstallCompositionTarget {
                            composition: composition.clone(),
                            inputs: CompositionOperationInputs::capture(
                                composition,
                                CompositionOperationKind::Update,
                            ),
                        });
                }
            }
        }
        Ok(context)
    }

    pub(super) fn apply_auto_install_updates(
        &mut self,
        updates: Vec<AutoInstallCompositionUpdate>,
    ) -> Result<Option<String>, String> {
        if updates.is_empty() {
            return Ok(None);
        }
        let previous_compositions = self.durable_state.compositions.compositions.clone();
        let previous_revision = self.durable_state.composition_revision;
        let mut updated = 0usize;
        let mut failures = Vec::new();
        for update in updates {
            let current = self
                .durable_state
                .compositions
                .compositions
                .iter()
                .find(|composition| composition.id == update.composition_id)
                .map(|composition| {
                    CompositionOperationInputs::capture(
                        composition,
                        CompositionOperationKind::Update,
                    )
                });
            if current.as_ref() != Some(&update.inputs) {
                failures.push(format!(
                    "composition '{}' changed during the Workshop download and was not changed by Auto-Install",
                    update.composition_id
                ));
                continue;
            }
            let failed = apply_operation_result_to_composition(
                &mut self.composition_preview,
                &mut self.durable_state.compositions.compositions,
                &update.composition_id,
                CompositionOperationKind::Update,
                CompositionOperationResult::Update(update.result),
            );
            if let Some(failure) = failed {
                failures.push(format!("{}: {failure}", update.composition_id));
            } else {
                updated += 1;
            }
        }
        let changed = self.durable_state.compositions.compositions != previous_compositions;
        if changed {
            self.durable_state.composition_revision = previous_revision.saturating_add(1);
            if let Some(store) = &self.state_store
                && let Err(error) = store.save(&self.durable_state)
            {
                self.durable_state.compositions.compositions = previous_compositions;
                self.durable_state.composition_revision = previous_revision;
                self.refresh_catalog();
                return Err(format!(
                    "Workshop content was downloaded, but Auto-Install could not save composition state: {error}"
                ));
            }
        }
        match self.collect_unreferenced_auto_install() {
            Ok(_) => {}
            Err(error) => failures.push(format!("Library cleanup: {error}")),
        }
        self.refresh_catalog();
        if failures.is_empty() {
            return Ok((updated > 0).then(|| {
                format!(
                    "Auto-Install updated {updated} dependent composition{}.",
                    if updated == 1 { "" } else { "s" }
                )
            }));
        }
        Err(format!(
            "Workshop content was downloaded, but Auto-Install could not update every dependent composition: {}",
            failures.join("; ")
        ))
    }

    pub(super) fn apply_workshop_completion(&mut self, completion: WorkshopOperationCompletion) {
        let mut succeeded = completion.result.is_ok();
        let mut failure = completion.result.as_ref().err().cloned();
        let mut summary = None;
        match completion.result {
            Ok(WorkshopOperationResult::Search(page)) => {
                summary = Some(format!(
                    "Steam Workshop returned {} items on page {} of {}.",
                    page.items.len(),
                    page.page,
                    page.total_results
                ));
                let revision = self
                    .workshop
                    .search
                    .as_ref()
                    .map_or(1, |search| search.revision.saturating_add(1));
                self.workshop.search = Some(workshop_search_snapshot(
                    match &completion.action {
                        WorkshopOperationKind::Search { query, .. } => query.clone(),
                        _ => String::new(),
                    },
                    page,
                    revision,
                ));
            }
            Ok(WorkshopOperationResult::Subscriptions(subscriptions)) => {
                self.fixture_workshop_subscriptions = subscriptions
                    .iter()
                    .map(|item| item.published_file_id.clone())
                    .collect();
                self.workshop.subscriptions = subscriptions
                    .iter()
                    .map(workshop_subscription_snapshot)
                    .collect();
                self.workshop.subscriptions_loaded = true;
                self.workshop.subscriptions_revision =
                    self.workshop.subscriptions_revision.saturating_add(1);
                summary = Some(format!(
                    "Loaded {} Steam Workshop subscriptions.",
                    subscriptions.len()
                ));
            }
            Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
                subscriptions,
                acquisition,
            }) => {
                self.fixture_workshop_subscriptions = subscriptions
                    .iter()
                    .map(|item| item.published_file_id.clone())
                    .collect();
                self.workshop.subscriptions = subscriptions
                    .iter()
                    .map(workshop_subscription_snapshot)
                    .collect();
                self.workshop.subscriptions_loaded = true;
                self.workshop.subscriptions_revision =
                    self.workshop.subscriptions_revision.saturating_add(1);
                match acquisition {
                    Ok((items, requested, updates)) => {
                        let revision = self
                            .workshop
                            .acquisition
                            .as_ref()
                            .map_or(1, |batch| batch.revision.saturating_add(1));
                        let items = items
                            .iter()
                            .map(workshop_acquisition_snapshot)
                            .collect::<Vec<_>>();
                        let imported = items.len();
                        self.fixture_acquisitions
                            .extend(items.iter().cloned().map(|item| (item.id.clone(), item)));
                        self.workshop.acquisition = Some(WorkshopAcquisitionBatchSnapshot {
                            revision,
                            requested,
                            items,
                        });
                        summary = Some(format!(
                            "Subscribed and added {imported} Workshop item{} to your Library.",
                            if imported == 1 { "" } else { "s" }
                        ));
                        self.refresh_catalog();
                        match self.apply_auto_install_updates(updates) {
                            Ok(Some(message)) => summary = Some(message),
                            Err(message) => {
                                succeeded = false;
                                failure = Some(message);
                            }
                            Ok(None) => {}
                        }
                    }
                    Err(error) => {
                        succeeded = false;
                        failure = Some(format!(
                            "Subscription is active, but Vapor Client could not add the item to your Library: {error}"
                        ));
                    }
                }
            }
            Ok(WorkshopOperationResult::Acquisitions {
                items,
                requested,
                updates,
            }) => {
                let revision = self
                    .workshop
                    .acquisition
                    .as_ref()
                    .map_or(1, |batch| batch.revision.saturating_add(1));
                let items = items
                    .iter()
                    .map(workshop_acquisition_snapshot)
                    .collect::<Vec<_>>();
                let imported = items.len();
                if self.bootstrap.fixture_mode {
                    self.fixture_acquisitions
                        .extend(items.iter().cloned().map(|item| (item.id.clone(), item)));
                }
                self.workshop.acquisition = Some(WorkshopAcquisitionBatchSnapshot {
                    revision,
                    requested,
                    items,
                });
                summary = Some(format!(
                    "Added {} Workshop items to your Library.",
                    imported
                ));
                self.refresh_catalog();
                match self.apply_auto_install_updates(updates) {
                    Ok(Some(message)) => summary = Some(message),
                    Err(message) => {
                        succeeded = false;
                        failure = Some(message);
                    }
                    Ok(None) => {}
                }
            }
            Ok(WorkshopOperationResult::Review(preview)) => {
                let (selector, target_triples, workspace_root) = match &completion.action {
                    WorkshopOperationKind::Stage {
                        selector,
                        target_triples,
                        workspace_root,
                    } => (
                        selector.clone(),
                        target_triples.clone(),
                        workspace_root.clone(),
                    ),
                    _ => (String::new(), Vec::new(), PathBuf::new()),
                };
                if self.bootstrap.workspace_root.as_deref() != Some(workspace_root.as_path()) {
                    succeeded = false;
                    failure = Some(
                        "The active workspace changed while this Workshop item was being staged. Stage it again from the current workspace.".to_owned(),
                    );
                } else {
                    self.workshop_review = Some(WorkshopReviewState {
                        review_id: completion.id.clone(),
                        selector,
                        target_triples,
                        workspace_root,
                        preview: preview.clone(),
                    });
                    summary = Some(format!(
                        "Staged {} · {} files · {} bytes for review.",
                        preview.id, preview.files, preview.bytes
                    ));
                }
            }
            Ok(WorkshopOperationResult::BatchReview(preview)) => {
                let (selectors, target_triples, workspace_root) = match &completion.action {
                    WorkshopOperationKind::StageBatch {
                        selectors,
                        target_triples,
                        workspace_root,
                    } => (
                        selectors.clone(),
                        target_triples.clone(),
                        workspace_root.clone(),
                    ),
                    _ => (Vec::new(), Vec::new(), PathBuf::new()),
                };
                if self.bootstrap.workspace_root.as_deref() != Some(workspace_root.as_path()) {
                    succeeded = false;
                    failure = Some(
                        "The active workspace changed while this Workshop batch was being staged. Stage it again from the current workspace.".to_owned(),
                    );
                } else if selectors != preview.selectors || selectors.len() != preview.items.len() {
                    succeeded = false;
                    failure = Some(
                        "The Workshop batch review did not match the selected content identities. Stage it again.".to_owned(),
                    );
                } else {
                    let total_files = preview.items.iter().map(|item| item.files).sum::<u64>();
                    let total_bytes = preview.items.iter().map(|item| item.bytes).sum::<u64>();
                    self.workshop_batch_review = Some(WorkshopBatchReviewState {
                        review_id: completion.id.clone(),
                        target_triples,
                        workspace_root,
                        preview: preview.clone(),
                    });
                    summary = Some(format!(
                        "Staged {} Workshop items · {total_files} files · {total_bytes} bytes for review.",
                        preview.items.len()
                    ));
                }
            }
            Ok(WorkshopOperationResult::Publication(publication)) => {
                self.workshop_review = None;
                summary = Some(format!(
                    "{} Workshop item {}{}; receipt saved at {}.",
                    if publication.created {
                        "Created"
                    } else {
                        "Updated"
                    },
                    publication.id,
                    publication
                        .published_file_id
                        .as_ref()
                        .map(|id| format!(" · PublishedFileId {id}"))
                        .unwrap_or_default(),
                    publication.receipt.display()
                ));
                self.refresh_catalog();
            }
            Ok(WorkshopOperationResult::BatchPublication(publications)) => {
                self.workshop_batch_review = None;
                let created = publications.iter().filter(|item| item.created).count();
                let updated = publications.len().saturating_sub(created);
                summary = Some(format!(
                    "Published {} Workshop items ({created} created, {updated} updated).",
                    publications.len()
                ));
                self.refresh_catalog();
            }
            Err(_) => {}
        }
        self.clear_active_operation();
        self.revision.0 = self.revision.0.saturating_add(1);
        let action = workshop_operation_action_id(&completion.action).to_owned();
        let message = failure.or(summary);
        self.operation_events
            .push_back(ClientEvent::OperationChanged(OperationEvent {
                revision: self.revision,
                id: completion.id,
                composition_id: None,
                action,
                phase: OperationPhase::Complete,
                status: if succeeded {
                    OperationStatus::Succeeded
                } else {
                    OperationStatus::Failed {
                        message: message
                            .clone()
                            .unwrap_or_else(|| "Workshop operation failed.".to_owned()),
                    }
                },
                progress: Some(1.0),
                message,
            }));
    }
}

fn parse_workshop_ids(ids: Vec<String>) -> Result<Vec<u64>, ServiceError> {
    if ids.is_empty() {
        return Err(invalid_client_action_request(
            "Select at least one Workshop item.",
        ));
    }
    if ids.len() > vapor_client_application::MAX_WORKSHOP_SUBSCRIPTION_BATCH {
        return Err(invalid_client_action_request(&format!(
            "At most {} Workshop items can be changed at once.",
            vapor_client_application::MAX_WORKSHOP_SUBSCRIPTION_BATCH
        )));
    }
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    for id in ids {
        let value = id.trim().parse::<u64>().ok().filter(|value| *value != 0);
        let Some(value) = value else {
            return Err(invalid_client_action_request(&format!(
                "Invalid Steam PublishedFileId: {}",
                id.trim()
            )));
        };
        if seen.insert(value) {
            parsed.push(value);
        }
    }
    Ok(parsed)
}

fn workshop_operation_action_id(kind: &WorkshopOperationKind) -> &'static str {
    match kind {
        WorkshopOperationKind::Search { .. } => "workshop.search",
        WorkshopOperationKind::LoadSubscriptions => "workshop.load-subscriptions",
        WorkshopOperationKind::ChangeSubscriptions {
            subscribe: true, ..
        } => "workshop.subscribe",
        WorkshopOperationKind::ChangeSubscriptions {
            subscribe: false, ..
        } => "workshop.unsubscribe",
        WorkshopOperationKind::Acquire { .. } => "workshop.acquire",
        WorkshopOperationKind::Stage { .. } => "workshop.stage",
        WorkshopOperationKind::StageBatch { .. } => "workshop.stage-batch",
        WorkshopOperationKind::Publish { .. } => "workshop.publish",
        WorkshopOperationKind::PublishBatch { .. } => "workshop.publish-batch",
    }
}

fn workshop_result_outcome(
    result: &Result<WorkshopOperationResult, String>,
) -> Result<String, String> {
    match result {
        Ok(WorkshopOperationResult::Search(page)) => Ok(format!(
            "Steam Workshop returned {} items on page {} of {}.",
            page.items.len(),
            page.page,
            page.total_results
        )),
        Ok(WorkshopOperationResult::Subscriptions(items)) => Ok(format!(
            "Loaded {} Steam Workshop subscriptions.",
            items.len()
        )),
        Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
            acquisition: Ok((items, requested, _updates)),
            ..
        }) => {
            if items.len() == *requested {
                Ok(format!(
                    "Subscribed and added {} Workshop items to the Library.",
                    items.len()
                ))
            } else {
                Err(format!(
                    "Subscribed, but only {} of {requested} Workshop items were added to the Library.",
                    items.len()
                ))
            }
        }
        Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
            acquisition: Err(error),
            ..
        }) => Err(format!(
            "Subscription is active, but Vapor Client could not add the item to the Library: {error}"
        )),
        Ok(WorkshopOperationResult::Acquisitions {
            items, requested, ..
        }) => {
            if items.len() == *requested {
                Ok(format!(
                    "Added {} Workshop items to the local cache.",
                    items.len()
                ))
            } else {
                Err(format!(
                    "Downloaded {} of {requested} requested Workshop items.",
                    items.len()
                ))
            }
        }
        Ok(WorkshopOperationResult::Review(preview)) => Ok(format!(
            "Staged {} · {} files · {} bytes for review.",
            preview.id, preview.files, preview.bytes
        )),
        Ok(WorkshopOperationResult::BatchReview(preview)) => Ok(format!(
            "Staged {} Workshop items for review.",
            preview.items.len()
        )),
        Ok(WorkshopOperationResult::Publication(publication)) => Ok(format!(
            "{} Workshop item {}{}; receipt saved at {}.",
            if publication.created {
                "Created"
            } else {
                "Updated"
            },
            publication.id,
            publication
                .published_file_id
                .as_ref()
                .map(|id| format!(" · PublishedFileId {id}"))
                .unwrap_or_default(),
            publication.receipt.display()
        )),
        Ok(WorkshopOperationResult::BatchPublication(publications)) => {
            Ok(format!("Published {} Workshop items.", publications.len()))
        }
        Err(error) => Err(error.clone()),
    }
}

fn execute_workshop_operation(
    application_executable: &Path,
    fixture_mode: bool,
    workshop_app_id: Option<u32>,
    fixture_subscriptions: BTreeSet<String>,
    auto_install: AutoInstallContext,
    kind: WorkshopOperationKind,
    context: application::OperationContext,
) -> Result<WorkshopOperationResult, String> {
    if fixture_mode {
        return Ok(fixture_workshop_operation(fixture_subscriptions, kind));
    }
    match kind {
        WorkshopOperationKind::Search { query, page } => {
            let app_id = required_workshop_app_id(workshop_app_id)?;
            vapor_client_application::search_workshop(app_id, &query, page)
                .map(WorkshopOperationResult::Search)
        }
        WorkshopOperationKind::LoadSubscriptions => {
            let app_id = required_workshop_app_id(workshop_app_id)?;
            vapor_client_application::workshop_subscriptions(app_id)
                .map(WorkshopOperationResult::Subscriptions)
        }
        WorkshopOperationKind::ChangeSubscriptions { ids, subscribe } => {
            let app_id = required_workshop_app_id(workshop_app_id)?;
            let change = if subscribe {
                vapor_client_application::subscribe_workshop_items(app_id, &ids)
            } else {
                vapor_client_application::unsubscribe_workshop_items(app_id, &ids)
            };
            change?;
            let subscriptions = vapor_client_application::workshop_subscriptions(app_id).map_err(|error| {
                format!("Subscription change completed, but the updated list could not be loaded: {error}")
            })?;
            if subscribe {
                let requested = ids.len();
                let ids = ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>();
                let acquisition = download_with_auto_install(
                    application_executable,
                    &ids,
                    requested,
                    &auto_install,
                );
                Ok(WorkshopOperationResult::SubscriptionWithAcquisition {
                    subscriptions,
                    acquisition,
                })
            } else {
                Ok(WorkshopOperationResult::Subscriptions(subscriptions))
            }
        }
        WorkshopOperationKind::Acquire { ids } => {
            let requested = ids.len();
            download_with_auto_install(application_executable, &ids, requested, &auto_install).map(
                |(items, _, updates)| WorkshopOperationResult::Acquisitions {
                    items,
                    requested,
                    updates,
                },
            )
        }
        WorkshopOperationKind::Stage {
            selector,
            target_triples,
            workspace_root,
        } => {
            context.set_phase(application::OperationPhase::Building);
            application::build_authored_workspace(
                application_executable,
                &workspace_root,
                &target_triples,
                application::BuildMode::ValidateAndBuild,
            )?;
            context.set_phase(application::OperationPhase::Staging);
            vapor_client_application::stage_workshop_preview(
                application_executable,
                &workspace_root,
                &selector,
                &target_triples,
            )
            .map(WorkshopOperationResult::Review)
        }
        WorkshopOperationKind::StageBatch {
            selectors,
            target_triples,
            workspace_root,
        } => {
            context.set_phase(application::OperationPhase::Building);
            application::build_authored_workspace(
                application_executable,
                &workspace_root,
                &target_triples,
                application::BuildMode::ValidateAndBuild,
            )?;
            context.set_phase(application::OperationPhase::Staging);
            vapor_client_application::stage_workshop_batch_preview(
                application_executable,
                &workspace_root,
                &selectors,
                &target_triples,
            )
            .map(WorkshopOperationResult::BatchReview)
        }
        WorkshopOperationKind::Publish {
            review,
            account,
            change_note,
        } => vapor_client_application::publish_reviewed_workshop_content(
            application_executable,
            &review.workspace_root,
            &review.selector,
            &review.preview,
            &review.target_triples,
            &account,
            change_note.as_deref(),
        )
        .map(WorkshopOperationResult::Publication),
        WorkshopOperationKind::PublishBatch {
            review,
            account,
            change_note,
        } => vapor_client_application::publish_reviewed_workshop_batch(
            application_executable,
            &review.workspace_root,
            &review.preview,
            &review.target_triples,
            &account,
            change_note.as_deref(),
        )
        .map(WorkshopOperationResult::BatchPublication),
    }
}

fn required_workshop_app_id(app_id: Option<u32>) -> Result<u32, String> {
    app_id.ok_or_else(|| {
        "This Vapor Client installation has no configured Workshop application.".to_owned()
    })
}

fn download_with_auto_install(
    application_executable: &Path,
    ids: &[String],
    requested: usize,
    context: &AutoInstallContext,
) -> Result<
    (
        Vec<vapor_client_application::WorkshopAcquisition>,
        usize,
        Vec<AutoInstallCompositionUpdate>,
    ),
    String,
> {
    let items = vapor_client_application::download_workshop_items_with_constraints(
        application_executable,
        ids,
        &context.constraints,
    )?;
    let acquired = items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut updates = Vec::new();
    let mut updated_compositions = BTreeSet::new();
    for (content_id, targets) in &context.compositions {
        if !acquired.contains(content_id.as_str()) {
            continue;
        }
        for target in targets {
            if !updated_compositions.insert(target.composition.id.clone()) {
                continue;
            }
            let selector = target
                .composition
                .root_package
                .as_ref()
                .map(|root| root.id.as_str())
                .ok_or_else(|| {
                    format!(
                        "Auto-Install could not update composition '{}' because it has no Packagepack.",
                        target.composition.name
                    )
                })?;
            let workspace = target
                .composition
                .workspace
                .as_ref()
                .map(|workspace| workspace.root.as_path());
            let result = vapor_client_application::update_composition(
                application_executable,
                workspace,
                selector,
            );
            updates.push(AutoInstallCompositionUpdate {
                composition_id: target.composition.id.clone(),
                inputs: target.inputs.clone(),
                result,
            });
        }
    }
    Ok((items, requested, updates))
}

fn fixture_workshop_operation(
    mut subscriptions: BTreeSet<String>,
    kind: WorkshopOperationKind,
) -> WorkshopOperationResult {
    let items = vec![
        fixture_workshop_item("1001", "Fixture Starter Pack"),
        fixture_workshop_item("1002", "Fixture Moon Pack"),
    ];
    match kind {
        WorkshopOperationKind::Search { query, page } => {
            let query = query.to_lowercase();
            let items = items
                .into_iter()
                .filter(|item| item.title.to_lowercase().contains(&query))
                .collect::<Vec<_>>();
            WorkshopOperationResult::Search(vapor_client_application::WorkshopPage {
                total_results: items.len() as u32,
                items,
                page,
            })
        }
        WorkshopOperationKind::LoadSubscriptions => WorkshopOperationResult::Subscriptions(
            subscriptions
                .iter()
                .map(|id| fixture_workshop_subscription(id))
                .collect(),
        ),
        WorkshopOperationKind::ChangeSubscriptions { ids, subscribe } => {
            let requested = ids.len();
            for id in &ids {
                if subscribe {
                    subscriptions.insert(id.to_string());
                } else {
                    subscriptions.remove(&id.to_string());
                }
            }
            let subscribed = subscriptions
                .iter()
                .map(|id| fixture_workshop_subscription(id))
                .collect();
            if subscribe {
                let items = ids
                    .into_iter()
                    .map(|id| vapor_client_application::WorkshopAcquisition {
                        id: format!("fixture.content.{id}"),
                        version: Some("1.0.0".to_owned()),
                        kind: "extension-mod".to_owned(),
                        cache_root: PathBuf::from(format!("fixture/library/fixture.content.{id}")),
                        fingerprint: format!("fixture:{id}"),
                    })
                    .collect();
                WorkshopOperationResult::SubscriptionWithAcquisition {
                    subscriptions: subscribed,
                    acquisition: Ok((items, requested, Vec::new())),
                }
            } else {
                WorkshopOperationResult::Subscriptions(subscribed)
            }
        }
        WorkshopOperationKind::Acquire { ids } => {
            let requested = ids.len();
            let items = ids
                .into_iter()
                .map(|id| vapor_client_application::WorkshopAcquisition {
                    id: format!("fixture.content.{id}"),
                    version: Some("1.0.0".to_owned()),
                    kind: "mod".to_owned(),
                    cache_root: PathBuf::from(format!("fixture/cache/fixture.content.{id}")),
                    fingerprint: format!("fixture:{id}"),
                })
                .collect();
            WorkshopOperationResult::Acquisitions {
                items,
                requested,
                updates: Vec::new(),
            }
        }
        WorkshopOperationKind::Stage {
            selector,
            target_triples,
            workspace_root: _,
        } => {
            let (id, version) = selector
                .split_once('@')
                .map(|(id, version)| (id.to_owned(), Some(version.to_owned())))
                .unwrap_or((selector, Some("1.0.0".to_owned())));
            WorkshopOperationResult::Review(vapor_client_application::WorkshopPackagePreview {
                id,
                version,
                app_id: Some(2122620),
                published_file_id: None,
                title: Some("Fixture Workshop artifact".to_owned()),
                visibility: Some("public".to_owned()),
                dependencies: Vec::new(),
                root: PathBuf::from("fixture/staged/artifact"),
                target: if target_triples.is_empty() {
                    "fixture".to_owned()
                } else {
                    target_triples.join(",")
                },
                fingerprint: "fixture:review".to_owned(),
                files: 1,
                bytes: 128,
                file_paths: vec!["Vapor.toml".to_owned()],
            })
        }
        WorkshopOperationKind::StageBatch {
            selectors,
            target_triples,
            workspace_root: _,
        } => {
            let reviewed_selectors = selectors.clone();
            let items = selectors
                .into_iter()
                .map(|selector| {
                    let (id, version) = selector
                        .split_once('@')
                        .map(|(id, version)| (id.to_owned(), Some(version.to_owned())))
                        .unwrap_or((selector, Some("1.0.0".to_owned())));
                    vapor_client_application::WorkshopPackagePreview {
                        id,
                        version,
                        app_id: Some(2122620),
                        published_file_id: None,
                        title: Some("Fixture Workshop artifact".to_owned()),
                        visibility: Some("public".to_owned()),
                        dependencies: Vec::new(),
                        root: PathBuf::from("fixture/staged/artifact"),
                        target: if target_triples.is_empty() {
                            "fixture".to_owned()
                        } else {
                            target_triples.join(",")
                        },
                        fingerprint: "fixture:review".to_owned(),
                        files: 1,
                        bytes: 128,
                        file_paths: vec!["Vapor.toml".to_owned()],
                    }
                })
                .collect();
            WorkshopOperationResult::BatchReview(vapor_client_application::WorkshopBatchReview {
                selectors: reviewed_selectors,
                items,
            })
        }
        WorkshopOperationKind::Publish { review, .. } => {
            WorkshopOperationResult::Publication(vapor_client_application::WorkshopPublication {
                id: review.preview.id.clone(),
                published_file_id: Some("1002".to_owned()),
                receipt: PathBuf::from("fixture/receipts/workshop.json"),
                created: review.preview.published_file_id.is_none(),
            })
        }
        WorkshopOperationKind::PublishBatch { review, .. } => {
            WorkshopOperationResult::BatchPublication(
                review
                    .preview
                    .items
                    .iter()
                    .map(|item| vapor_client_application::WorkshopPublication {
                        id: item.id.clone(),
                        published_file_id: Some("1002".to_owned()),
                        receipt: PathBuf::from("fixture/receipts/workshop.json"),
                        created: item.published_file_id.is_none(),
                    })
                    .collect(),
            )
        }
    }
}

fn fixture_workshop_item(id: &str, title: &str) -> vapor_client_application::WorkshopItem {
    vapor_client_application::WorkshopItem {
        published_file_id: id.to_owned(),
        title: title.to_owned(),
        description: "Deterministic Vapor Client fixture; no Steam connection is used.".to_owned(),
        preview_url: None,
        updated_at_unix: 1_797_000_000,
    }
}

fn fixture_workshop_subscription(id: &str) -> vapor_client_application::WorkshopSubscription {
    let item = match id {
        "1001" => fixture_workshop_item(id, "Fixture Starter Pack"),
        "1002" => fixture_workshop_item(id, "Fixture Moon Pack"),
        _ => fixture_workshop_item(id, &format!("Fixture Workshop item {id}")),
    };
    vapor_client_application::WorkshopSubscription {
        published_file_id: item.published_file_id,
        title: item.title,
        description: item.description,
        preview_url: item.preview_url,
        updated_at_unix: item.updated_at_unix,
        installed: false,
        needs_update: false,
        downloading: false,
        download_progress: None,
    }
}

fn workshop_search_snapshot(
    query: String,
    page: vapor_client_application::WorkshopPage,
    revision: u64,
) -> WorkshopSearchSnapshot {
    WorkshopSearchSnapshot {
        revision,
        query,
        page: page.page,
        total_results: page.total_results,
        items: page.items.into_iter().map(workshop_item_snapshot).collect(),
    }
}

fn workshop_item_snapshot(item: vapor_client_application::WorkshopItem) -> WorkshopItemSnapshot {
    WorkshopItemSnapshot {
        published_file_id: item.published_file_id,
        title: item.title,
        description: item.description,
        preview_url: item.preview_url,
        updated_at_unix: item.updated_at_unix,
    }
}

fn workshop_subscription_snapshot(
    item: &vapor_client_application::WorkshopSubscription,
) -> WorkshopSubscriptionSnapshot {
    WorkshopSubscriptionSnapshot {
        published_file_id: item.published_file_id.clone(),
        title: item.title.clone(),
        description: item.description.clone(),
        preview_url: item.preview_url.clone(),
        updated_at_unix: item.updated_at_unix,
        installed: item.installed,
        needs_update: item.needs_update,
        downloading: item.downloading,
        download_progress: item.download_progress,
    }
}

fn workshop_acquisition_snapshot(
    item: &vapor_client_application::WorkshopAcquisition,
) -> WorkshopAcquisitionSnapshot {
    WorkshopAcquisitionSnapshot {
        id: item.id.clone(),
        version: item.version.clone(),
        kind: item.kind.clone(),
        cache_root: item.cache_root.display().to_string(),
        fingerprint: item.fingerprint.clone(),
    }
}

pub(super) fn workshop_review_snapshot(review: &WorkshopReviewState) -> WorkshopReviewSnapshot {
    let preview = &review.preview;
    let visible_paths = preview
        .file_paths
        .iter()
        .take(MAX_WORKSHOP_REVIEW_FILE_PATHS)
        .cloned()
        .collect::<Vec<_>>();
    WorkshopReviewSnapshot {
        review_id: review.review_id.clone(),
        selector: review.selector.clone(),
        preview: WorkshopPackagePreviewSnapshot {
            id: preview.id.clone(),
            version: preview.version.clone(),
            app_id: preview.app_id,
            published_file_id: preview.published_file_id.clone(),
            title: preview.title.clone(),
            visibility: preview.visibility.clone(),
            dependencies: preview.dependencies.clone(),
            root: preview.root.display().to_string(),
            target: preview.target.clone(),
            fingerprint: preview.fingerprint.clone(),
            files: preview.files,
            bytes: preview.bytes,
            omitted_file_paths: preview.file_paths.len().saturating_sub(visible_paths.len()),
            file_paths: visible_paths,
        },
    }
}

pub(super) fn workshop_batch_review_snapshot(
    review: &WorkshopBatchReviewState,
) -> vapor_client_protocol::WorkshopBatchReviewSnapshot {
    vapor_client_protocol::WorkshopBatchReviewSnapshot {
        review_id: review.review_id.clone(),
        items: review
            .preview
            .selectors
            .iter()
            .zip(&review.preview.items)
            .map(|(selector, preview)| {
                let item = WorkshopReviewState {
                    review_id: review.review_id.clone(),
                    selector: selector.clone(),
                    target_triples: review.target_triples.clone(),
                    workspace_root: review.workspace_root.clone(),
                    preview: preview.clone(),
                };
                let snapshot = workshop_review_snapshot(&item);
                vapor_client_protocol::WorkshopReviewItemSnapshot {
                    selector: snapshot.selector,
                    preview: snapshot.preview,
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod target_tests {
    use super::super::validate_runtime_target_matrix;

    #[test]
    fn workshop_review_accepts_an_empty_or_unique_target_matrix() {
        assert!(validate_runtime_target_matrix(&[]).is_ok());
        assert!(
            validate_runtime_target_matrix(&[
                "x86_64-unknown-linux-gnu".to_owned(),
                "x86_64-pc-windows-msvc".to_owned(),
            ])
            .is_ok()
        );
    }

    #[test]
    fn workshop_review_rejects_ambiguous_or_unbounded_target_matrices() {
        let duplicate = vec!["x86_64-unknown-linux-gnu".to_owned(); 2];
        assert!(validate_runtime_target_matrix(&duplicate).is_err());

        let too_many = vec!["target".to_owned(); 33];
        assert!(validate_runtime_target_matrix(&too_many).is_err());
        assert!(validate_runtime_target_matrix(&[" target".to_owned()]).is_err());
        assert!(validate_runtime_target_matrix(&["".to_owned()]).is_err());
        assert!(validate_runtime_target_matrix(&["x".repeat(129)]).is_err());
    }
}
