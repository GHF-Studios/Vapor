//! Steam client services used by Vapor frontends.
//!
//! Steam API handles stay inside the calling operation. Public results are
//! owned data so a UI can safely keep them after Steam callbacks complete.

use std::{sync::mpsc, thread, time::{Duration, Instant}};
use steamworks::{AppId, AppIDs, Client, PublishedFileId, SteamError, UGCQueryType, UGCType};

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);
const CALLBACK_POLL: Duration = Duration::from_millis(10);
const MAX_PAGE: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopItem {
    pub published_file_id: String,
    pub title: String,
    pub description: String,
    pub preview_url: Option<String>,
    pub updated_at_unix: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopPage {
    pub items: Vec<WorkshopItem>,
    pub page: u32,
    pub total_results: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSubscription {
    pub published_file_id: String,
}

/// Search the app's public Workshop through Steamworks UGC.
///
/// Steam client initialization and callback pumping are local to this call;
/// callers should run it on their existing background-operation worker.
pub fn search_workshop(
    app_id: u32,
    search_text: &str,
    page: u32,
) -> Result<WorkshopPage, String> {
    if app_id == 0 {
        return Err("Steam Workshop search needs a nonzero application ID".to_owned());
    }
    if !(1..=MAX_PAGE).contains(&page) {
        return Err(format!("Steam Workshop page must be between 1 and {MAX_PAGE}"));
    }

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

    let (sender, receiver) = mpsc::channel();
    query.fetch(move |result| {
        let mapped = result.map(|results| {
            let total_results = results.total_results();
            let items = results
                .iter()
                .enumerate()
                .filter_map(|(index, item)| item.map(|item| WorkshopItem {
                    published_file_id: item.published_file_id.0.to_string(),
                    title: item.title,
                    description: item.description,
                    preview_url: results.preview_url(index as u32),
                    updated_at_unix: item.time_updated,
                }))
                .collect();
            WorkshopPage { items, page, total_results }
        }).map_err(|error| format!("Steam Workshop query failed: {error:?}"));
        let _ = sender.send(mapped);
    });

    pump_callbacks(&client, receiver)
}

/// Read the current account's subscriptions for the app.
pub fn subscribed_items(app_id: u32) -> Result<Vec<WorkshopSubscription>, String> {
    let client = initialize_client(app_id)?;
    Ok(client.ugc().subscribed_items(true).into_iter().map(|id| {
        WorkshopSubscription { published_file_id: id.0.to_string() }
    }).collect())
}

/// Subscribe the current Steam account to one Workshop item.
pub fn subscribe_item(app_id: u32, published_file_id: u64) -> Result<(), String> {
    change_subscription(app_id, PublishedFileId(published_file_id), true)
}

/// Unsubscribe the current Steam account from one Workshop item.
pub fn unsubscribe_item(app_id: u32, published_file_id: u64) -> Result<(), String> {
    change_subscription(app_id, PublishedFileId(published_file_id), false)
}

fn change_subscription(
    app_id: u32,
    published_file_id: PublishedFileId,
    subscribe: bool,
) -> Result<(), String> {
    let client = initialize_client(app_id)?;
    let ugc = client.ugc();
    let (sender, receiver) = mpsc::channel();
    let callback = move |result: Result<(), SteamError>| {
        let _ = sender.send(result.map_err(|error| format!("Steam subscription action failed: {error:?}")));
    };
    if subscribe {
        ugc.subscribe_item(published_file_id, callback);
    } else {
        ugc.unsubscribe_item(published_file_id, callback);
    }
    pump_callbacks(&client, receiver)
}

fn initialize_client(app_id: u32) -> Result<Client, String> {
    if app_id == 0 {
        return Err("Steam Workshop needs a nonzero application ID".to_owned());
    }
    Client::init_app(AppId(app_id)).map_err(|error| {
        format!("Steamworks is unavailable for app {app_id}: {error:?}")
    })
}

fn pump_callbacks<T>(client: &Client, receiver: mpsc::Receiver<Result<T, String>>) -> Result<T, String> {
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
                return Err("Steamworks did not complete the Workshop operation within 30 seconds".to_owned());
            }
        }
    }
}
