use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub struct PageRecord {
    pub url_path: String,
    pub file_path: String,
    pub title: String,
    pub parent_path: String,
    pub depth: i32,
    pub is_dir_index: bool,
    pub content_hash: [u8; 16],
    pub frontmatter_json: String,
    pub parsed_html: String,
    pub raw_content: String,
    pub taxonomy: String,
    pub links: Vec<String>,
}

#[derive(Debug)]
pub enum DbWriteCommand {
    UpsertPage {
        page: Box<PageRecord>,
        resp: Option<oneshot::Sender<Result<(), String>>>,
    },
    RemoveFile {
        file_path: PathBuf,
        resp: Option<oneshot::Sender<Result<(), String>>>,
    },
    PruneDeletedPages {
        active_files: Vec<String>,
        resp: Option<oneshot::Sender<Result<usize, String>>>,
    },
    RebuildDirectoryTree {
        resp: Option<oneshot::Sender<Result<(), String>>>,
    },
    UpdateDirectoryTitle {
        url_path: String,
        title: String,
        resp: Option<oneshot::Sender<Result<(), String>>>,
    },
    GetAllPagesForHooks {
        resp: oneshot::Sender<Result<Vec<crate::scripts::PageHookEntity>, String>>,
    },
    GetAllDirectoriesForHooks {
        resp: oneshot::Sender<Result<Vec<crate::scripts::DirectoryHookEntity>, String>>,
    },
    UpdatePageFromHook {
        entity: Box<crate::scripts::PageHookEntity>,
        resp: Option<oneshot::Sender<Result<(), String>>>,
    },
    ExecuteRaw {
        sql: String,
        resp: Option<oneshot::Sender<Result<usize, String>>>,
    },
}

#[derive(Clone)]
pub struct DbWriterHandle {
    tx: mpsc::Sender<DbWriteCommand>,
}

impl DbWriterHandle {
    pub fn new(tx: mpsc::Sender<DbWriteCommand>) -> Self {
        Self { tx }
    }

    pub async fn send(&self, cmd: DbWriteCommand) -> Result<(), mpsc::error::SendError<DbWriteCommand>> {
        self.tx.send(cmd).await
    }

    pub async fn upsert_page(&self, page: PageRecord) -> Result<(), String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::UpsertPage {
                page: Box::new(page),
                resp: Some(resp_tx),
            })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }

    pub async fn remove_file(&self, file_path: PathBuf) -> Result<(), String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::RemoveFile {
                file_path,
                resp: Some(resp_tx),
            })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }

    pub async fn prune_deleted_pages(&self, active_files: Vec<String>) -> Result<usize, String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::PruneDeletedPages {
                active_files,
                resp: Some(resp_tx),
            })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }

    pub async fn rebuild_directory_tree(&self) -> Result<(), String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::RebuildDirectoryTree {
                resp: Some(resp_tx),
            })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }

    pub async fn update_directory_title(&self, url_path: String, title: String) -> Result<(), String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::UpdateDirectoryTitle {
                url_path,
                title,
                resp: Some(resp_tx),
            })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }

    pub async fn get_all_pages_for_hooks(&self) -> Result<Vec<crate::scripts::PageHookEntity>, String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::GetAllPagesForHooks { resp: resp_tx })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }

    pub async fn get_all_directories_for_hooks(&self) -> Result<Vec<crate::scripts::DirectoryHookEntity>, String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::GetAllDirectoriesForHooks { resp: resp_tx })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }

    pub async fn update_page_from_hook(&self, entity: crate::scripts::PageHookEntity) -> Result<(), String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(DbWriteCommand::UpdatePageFromHook {
                entity: Box::new(entity),
                resp: Some(resp_tx),
            })
            .await
            .map_err(|e| e.to_string())?;
        resp_rx.await.map_err(|e| e.to_string())?
    }
}
