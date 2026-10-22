//! Steam client services used by Vapor frontends.
//!
//! Steam API handles stay inside the calling operation. Public results are
//! owned data so a UI can safely keep them after Steam callbacks complete.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use steamworks::{
    AppIDs, AppId, Client, DownloadItemResult, ItemState, PublishedFileId, SteamError,
    UGCQueryType, UGCType,
};

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);
const CALLBACK_POLL: Duration = Duration::from_millis(10);
const MAX_PAGE: u32 = 1_000;
const WORKSHOP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Maximum number of subscription changes sent through one client session.
pub const MAX_SUBSCRIPTION_BATCH_SIZE: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopItem {
    pub published_file_id: String,
    pub title: String,
    pub description: String,
    pub preview_url: Option<String>,
    pub updated_at_unix: u32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopPage {
    pub items: Vec<WorkshopItem>,
    pub page: u32,
    pub total_results: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkshopSubscriptionDetails {
    title: String,
    description: String,
    preview_url: Option<String>,
    updated_at_unix: u32,
    tags: Vec<String>,
}

fn workshop_subscription_details(item: Option<WorkshopItem>) -> WorkshopSubscriptionDetails {
    item.map_or_else(
        || WorkshopSubscriptionDetails {
            title: "Workshop item details unavailable".to_owned(),
            description: String::new(),
            preview_url: None,
            updated_at_unix: 0,
            tags: Vec::new(),
        },
        |item| WorkshopSubscriptionDetails {
            title: item.title,
            description: item.description,
            preview_url: item.preview_url,
            updated_at_unix: item.updated_at_unix,
            tags: item.tags,
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSubscription {
    pub published_file_id: String,
    pub title: String,
    pub description: String,
    pub preview_url: Option<String>,
    pub updated_at_unix: u32,
    pub tags: Vec<String>,
    pub installed: bool,
    pub needs_update: bool,
    pub downloading: bool,
    pub download_progress: Option<(u64, u64)>,
}

/// Download Workshop items and consume each local folder while Steam keeps it available.
///
/// The current user's subscription list is not changed. Steam keeps downloads
/// temporary for items the user has not subscribed to. The folder path is only
/// valid for the callback; copy any files that must outlive this call.
pub fn with_downloaded_workshop_items<T>(
    app_id: u32,
    published_file_ids: &[u64],
    consume: impl FnMut(u64, &Path) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    with_workshop_items(app_id, published_file_ids, false, consume)
}

/// Refresh Workshop items from Steam before consuming their current local folders.
///
/// Unlike [`with_downloaded_workshop_items`], this asks Steam to update items that
/// are already installed and currently appear up to date. Explicit composition
/// updates use this path so Steam remains the authority on the newest payload.
///
/// # Errors
///
/// Returns Steam initialization, download, timeout, or consumer errors.
pub fn with_refreshed_workshop_items<T>(
    app_id: u32,
    published_file_ids: &[u64],
    consume: impl FnMut(u64, &Path) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    with_workshop_items(app_id, published_file_ids, true, consume)
}

fn with_workshop_items<T>(
    app_id: u32,
    published_file_ids: &[u64],
    refresh_installed: bool,
    mut consume: impl FnMut(u64, &Path) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    if app_id == 0 {
        return Err("Steam Workshop download needs a nonzero application ID".to_owned());
    }
    if published_file_ids.is_empty() {
        return Err("at least one Workshop item is required".to_owned());
    }
    if published_file_ids.contains(&0) {
        return Err("Workshop item IDs must be nonzero".to_owned());
    }

    let client = initialize_client(app_id)?;
    let ugc = client.ugc();
    let mut consumed = Vec::with_capacity(published_file_ids.len());
    let mut seen = BTreeSet::new();
    for published_file_id in published_file_ids {
        if !seen.insert(*published_file_id) {
            continue;
        }
        let item = PublishedFileId(*published_file_id);
        let state = ugc.item_state(item);
        if should_reuse_local_item(state, refresh_installed)
            && let Some(info) = ugc.item_install_info(item)
        {
            consumed.push(consume(*published_file_id, Path::new(&info.folder))?);
            continue;
        }

        let (sender, receiver) = mpsc::channel();
        let callback = client.register_callback(move |result: DownloadItemResult| {
            if result.app_id.0 == app_id && result.published_file_id == item {
                let result = result
                    .error
                    .map_or(Ok(()), |error| Err(format!("{error:?}")));
                let _ = sender.send(result);
            }
        });
        if !ugc.download_item(item, false) {
            return Err(format!(
                "Steam could not start downloading Workshop item {published_file_id}; check that Steam is signed in and the item is available to this account"
            ));
        }

        let deadline = Instant::now() + WORKSHOP_DOWNLOAD_TIMEOUT;
        let result = loop {
            client.run_callbacks();
            match receiver.try_recv() {
                Ok(result) => break result,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(format!(
                        "Steam stopped reporting the download for Workshop item {published_file_id}"
                    ));
                }
                Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::sleep(CALLBACK_POLL);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    return Err(format!(
                        "timed out while Steam downloaded Workshop item {published_file_id}"
                    ));
                }
            }
        };
        drop(callback);
        result.map_err(|error| {
            format!(
                "Steam could not download Workshop item {published_file_id} ({error}). Check that Steam is open and signed in, and that this account can access the item on the game's Workshop page."
            )
        })?;
        let root = installed_workshop_item_root(&ugc, item)?;
        consumed.push(consume(*published_file_id, &root)?);
    }
    Ok(consumed)
}

fn should_reuse_local_item(state: ItemState, refresh_installed: bool) -> bool {
    !refresh_installed
        && state.contains(ItemState::INSTALLED)
        && !state.contains(ItemState::NEEDS_UPDATE)
}

fn installed_workshop_item_root(
    ugc: &steamworks::UGC,
    item: PublishedFileId,
) -> Result<PathBuf, String> {
    ugc.item_install_info(item)
        .map(|info| PathBuf::from(info.folder))
        .ok_or_else(|| {
            format!(
                "Steam completed the download for Workshop item {}, but provided no local folder",
                item.0
            )
        })
}

/// Search the app's public Workshop through Steamworks UGC.
///
/// Steam client initialization and callback pumping are local to this call;
/// callers should run it on their existing background-operation worker.
pub fn search_workshop(
    app_id: u32,
    search_text: &str,
    page: u32,
    content_kind: Option<&str>,
    required_tags: &[String],
) -> Result<WorkshopPage, String> {
    if app_id == 0 {
        return Err("Steam Workshop search needs a nonzero application ID".to_owned());
    }
    if !(1..=MAX_PAGE).contains(&page) {
        return Err(format!(
            "Steam Workshop page must be between 1 and {MAX_PAGE}"
        ));
    }
    let content_kind = content_kind.map(str::to_owned);

    let client = initialize_client(app_id)?;
    let ugc = client.ugc();
    let query_type = if search_text.trim().is_empty() {
        UGCQueryType::RankedByPublicationDate
    } else {
        UGCQueryType::RankedByTextSearch
    };
    let mut query = ugc
        .query_all(
            query_type,
            UGCType::Items,
            AppIDs::ConsumerAppId(AppId(app_id)),
            page,
        )
        .map_err(|error| format!("could not create Steam Workshop query: {error:?}"))?
        .include_long_desc(true)
        .allow_cached_response(60);
    if !search_text.trim().is_empty() {
        query = query.set_search_text(search_text.trim());
    }
    let tags = canonical_search_tags(content_kind.as_deref(), required_tags);
    for tag in &tags {
        let tag = tag.trim();
        if !tag.is_empty() {
            query = query.require_tag(tag);
        }
    }

    let (sender, receiver) = mpsc::channel();
    query.fetch(move |result| {
        let mapped = result
            .map(|results| {
                let total_results = results.total_results();
                let items = results
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        item.map(|item| WorkshopItem {
                            published_file_id: item.published_file_id.0.to_string(),
                            title: item.title,
                            description: item.description,
                            preview_url: results.preview_url(index as u32),
                            updated_at_unix: item.time_updated,
                            tags: normalize_tags(item.tags),
                        })
                        .filter(|item| item_matches_search_tags(&item.tags, &tags))
                    })
                    .collect();
                WorkshopPage {
                    items,
                    page,
                    total_results,
                }
            })
            .map_err(|error| format!("Steam Workshop query failed: {error:?}"));
        let _ = sender.send(mapped);
    });

    pump_callbacks(&client, receiver)
}

fn canonical_search_tags(content_kind: Option<&str>, required_tags: &[String]) -> Vec<String> {
    let mut tags = Vec::with_capacity(required_tags.len() + usize::from(content_kind.is_some()));
    if let Some(content_kind) = content_kind.map(str::trim).filter(|tag| !tag.is_empty()) {
        tags.push(content_kind.to_owned());
    }
    tags.extend(
        required_tags
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned),
    );
    tags.sort_by_key(|tag| tag.to_ascii_lowercase());
    tags.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    tags
}

fn item_matches_search_tags(item_tags: &[String], required_tags: &[String]) -> bool {
    required_tags.iter().all(|required| {
        item_tags
            .iter()
            .any(|tag| tag.trim().eq_ignore_ascii_case(required.trim()))
    })
}

/// Read the current account's subscriptions for the app.
pub fn subscribed_items(app_id: u32) -> Result<Vec<WorkshopSubscription>, String> {
    let client = initialize_client(app_id)?;
    let ugc = client.ugc();
    let ids = ugc.subscribed_items(true);
    let mut states = ids
        .iter()
        .copied()
        .map(|id| {
            let state = ugc.item_state(id);
            (
                id,
                (
                    state.contains(ItemState::INSTALLED),
                    state.contains(ItemState::NEEDS_UPDATE),
                    state.intersects(ItemState::DOWNLOADING | ItemState::DOWNLOAD_PENDING),
                    ugc.item_download_info(id),
                ),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut details = std::collections::BTreeMap::new();

    for chunk in ids.chunks(50) {
        let query = ugc
            .query_items(chunk.to_vec())
            .map_err(|error| format!("could not query subscribed Workshop items: {error}"))?
            .include_long_desc(true)
            .allow_cached_response(60);
        let (sender, receiver) = mpsc::channel();
        query.fetch(move |result| {
            let mapped = result
                .map(|results| {
                    results
                        .iter()
                        .enumerate()
                        .filter_map(|(index, item)| {
                            item.map(|item| WorkshopItem {
                                published_file_id: item.published_file_id.0.to_string(),
                                title: item.title,
                                description: item.description,
                                preview_url: results.preview_url(index as u32),
                                updated_at_unix: item.time_updated,
                                tags: normalize_tags(item.tags),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .map_err(|error| format!("subscribed Workshop item query failed: {error:?}"));
            let _ = sender.send(mapped);
        });
        for item in pump_callbacks(&client, receiver)? {
            details.insert(item.published_file_id.clone(), item);
        }
    }

    Ok(ids
        .into_iter()
        .map(|id| {
            let published_file_id = id.0.to_string();
            let item = details.remove(&published_file_id);
            let (installed, needs_update, downloading, download_progress) =
                states.remove(&id).unwrap_or_default();
            let details = workshop_subscription_details(item);
            WorkshopSubscription {
                published_file_id,
                title: details.title,
                description: details.description,
                preview_url: details.preview_url,
                updated_at_unix: details.updated_at_unix,
                tags: details.tags,
                installed,
                needs_update,
                downloading,
                download_progress,
            }
        })
        .collect())
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    tags.into_iter()
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// Subscribe the current Steam account to one Workshop item.
pub fn subscribe_item(app_id: u32, published_file_id: u64) -> Result<(), String> {
    change_subscriptions(app_id, &[published_file_id], true)
}

/// Unsubscribe the current Steam account from one Workshop item.
pub fn unsubscribe_item(app_id: u32, published_file_id: u64) -> Result<(), String> {
    change_subscriptions(app_id, &[published_file_id], false)
}

/// Subscribe the current account to several Workshop items in one Steam session.
pub fn subscribe_items(app_id: u32, published_file_ids: &[u64]) -> Result<(), String> {
    change_subscriptions(app_id, published_file_ids, true)
}

/// Unsubscribe the current account from several Workshop items in one Steam session.
pub fn unsubscribe_items(app_id: u32, published_file_ids: &[u64]) -> Result<(), String> {
    change_subscriptions(app_id, published_file_ids, false)
}

fn change_subscriptions(
    app_id: u32,
    published_file_ids: &[u64],
    subscribe: bool,
) -> Result<(), String> {
    if published_file_ids.is_empty() {
        return Err("at least one Workshop item is required".to_owned());
    }
    if published_file_ids.len() > MAX_SUBSCRIPTION_BATCH_SIZE {
        return Err(format!(
            "at most {MAX_SUBSCRIPTION_BATCH_SIZE} Workshop items can be changed at once"
        ));
    }
    if published_file_ids.contains(&0) {
        return Err("Workshop item IDs must be nonzero".to_owned());
    }
    let mut seen = std::collections::BTreeSet::new();
    let ids = published_file_ids
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect::<Vec<_>>();
    let client = initialize_client(app_id)?;
    let ugc = client.ugc();
    let (sender, receiver) = mpsc::channel();
    for id in &ids {
        let sender = sender.clone();
        let published_file_id = PublishedFileId(*id);
        let callback = move |result: Result<(), SteamError>| {
            let _ = sender.send(
                result.map_err(|error| format!("Workshop item {}: {error:?}", published_file_id.0)),
            );
        };
        if subscribe {
            ugc.subscribe_item(published_file_id, callback);
        } else {
            ugc.unsubscribe_item(published_file_id, callback);
        }
    }
    drop(sender);
    pump_subscription_callbacks(&client, receiver, ids.len())
}

fn pump_subscription_callbacks(
    client: &Client,
    receiver: mpsc::Receiver<Result<(), String>>,
    expected: usize,
) -> Result<(), String> {
    let deadline = Instant::now() + CALLBACK_TIMEOUT;
    let mut completed = 0;
    let mut failures = Vec::new();
    while completed < expected {
        client.run_callbacks();
        match receiver.try_recv() {
            Ok(Ok(())) => completed += 1,
            Ok(Err(error)) => {
                completed += 1;
                failures.push(error);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(format!(
                    "Steamworks completed {completed} of {expected} subscription actions before callbacks ended"
                ));
            }
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(CALLBACK_POLL);
            }
            Err(mpsc::TryRecvError::Empty) => {
                return Err(format!(
                    "timed out after {completed} of {expected} Workshop subscription actions"
                ));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} of {expected} Workshop subscription actions failed: {}",
            failures.len(),
            failures.join("; ")
        ))
    }
}

fn initialize_client(app_id: u32) -> Result<Client, String> {
    if app_id == 0 {
        return Err("Steam Workshop needs a nonzero application ID".to_owned());
    }
    Client::init_app(AppId(app_id))
        .map_err(|error| {
            format!(
                "Steam Workshop could not connect for app {app_id}. Open Steam, sign in, and try again ({error:?})."
            )
        })
}

fn pump_callbacks<T>(
    client: &Client,
    receiver: mpsc::Receiver<Result<T, String>>,
) -> Result<T, String> {
    let deadline = Instant::now() + CALLBACK_TIMEOUT;
    loop {
        client.run_callbacks();
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("Steamworks callback ended without a result".to_owned());
            }
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(CALLBACK_POLL);
            }
            Err(mpsc::TryRecvError::Empty) => {
                return Err(
                    "Steamworks did not complete the Workshop operation within 30 seconds"
                        .to_owned(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_search_tags, item_matches_search_tags, should_reuse_local_item};
    use steamworks::ItemState;

    #[test]
    fn explicit_refresh_never_reuses_an_installed_copy() {
        let current = ItemState::INSTALLED;
        let outdated = ItemState::INSTALLED | ItemState::NEEDS_UPDATE;

        assert!(should_reuse_local_item(current, false));
        assert!(!should_reuse_local_item(current, true));
        assert!(!should_reuse_local_item(outdated, false));
        assert!(!should_reuse_local_item(ItemState::NONE, false));
    }

    #[test]
    fn subscription_details_preserve_steam_tags() {
        let details = super::workshop_subscription_details(Some(super::WorkshopItem {
            published_file_id: "123".to_owned(),
            title: "Playable pack".to_owned(),
            description: String::new(),
            preview_url: None,
            updated_at_unix: 1,
            tags: vec!["packagepack".to_owned(), "space".to_owned()],
        }));

        assert_eq!(details.tags, ["packagepack", "space"]);
    }

    #[test]
    fn content_kind_is_a_required_canonical_tag_separate_from_user_tags() {
        let tags = canonical_search_tags(
            Some("game-mod"),
            &["  survival ".to_owned(), "GAME-MOD".to_owned()],
        );
        assert_eq!(tags, ["game-mod", "survival"]);
        assert!(item_matches_search_tags(
            &["Game-Mod".to_owned(), "Survival".to_owned()],
            &tags
        ));
        assert!(!item_matches_search_tags(
            &["game".to_owned(), "survival".to_owned()],
            &tags
        ));
    }
}
