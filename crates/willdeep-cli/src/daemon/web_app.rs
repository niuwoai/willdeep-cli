//! Runtime-owned Web endpoint. The terminal owns no listener or process.
use super::*;
use crate::i18n::Language;
use willdeep_runtime_protocol::web_app::{WebAppStartParams, WebAppStatus};

#[cfg(test)]
mod tests;

pub(crate) async fn remote_status(home: &Path) -> Result<WebAppStatus> {
    if let Ok(state) = load_state(&DaemonPaths::new(home).state)
        && probe(&state).await.is_ok()
    {
        return runtime_client(&state)?
            .web_app_status()
            .await?
            .into_result()
            .map_err(Into::into);
    }
    let saved = load(home)?;
    Ok(WebAppStatus {
        enabled: saved.as_ref().is_some_and(|settings| settings.enabled),
        listen: saved.map(|settings| settings.request.listen),
        ..Default::default()
    })
}

pub(crate) async fn remote_start(home: &Path, request: &WebAppStartParams) -> Result<WebAppStatus> {
    let state = ensure_running(home).await?;
    runtime_client(&state)?
        .web_app_start(request)
        .await?
        .into_result()
        .map_err(Into::into)
}

pub(crate) async fn remote_stop(home: &Path) -> Result<WebAppStatus> {
    if let Ok(state) = load_state(&DaemonPaths::new(home).state)
        && probe(&state).await.is_ok()
    {
        return runtime_client(&state)?
            .web_app_stop()
            .await?
            .into_result()
            .map_err(Into::into);
    }
    if let Some(mut settings) = load(home)? {
        settings.enabled = false;
        save(home, &settings)?;
    }
    Ok(WebAppStatus::default())
}

#[derive(Serialize, Deserialize)]
struct Settings {
    enabled: bool,
    request: WebAppStartParams,
}

struct Serving {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    ready: tokio::task::JoinHandle<()>,
}

pub(super) struct WebAppService {
    home: PathBuf,
    status: Arc<std::sync::Mutex<WebAppStatus>>,
    serving: AsyncMutex<Option<Serving>>,
}

impl WebAppService {
    pub(super) fn new(home: &Path) -> Self {
        let initial = match load(home) {
            Ok(Some(settings)) => WebAppStatus {
                enabled: settings.enabled,
                listen: Some(settings.request.listen),
                ..Default::default()
            },
            Ok(None) => WebAppStatus::default(),
            Err(_) => WebAppStatus {
                error: Some("Web App settings could not be read".into()),
                ..Default::default()
            },
        };
        Self {
            home: home.into(),
            status: Arc::new(std::sync::Mutex::new(initial)),
            serving: AsyncMutex::new(None),
        }
    }

    pub(super) fn status(&self) -> WebAppStatus {
        self.status.lock().expect("Web App status").clone()
    }

    pub(super) async fn start(&self, mut request: WebAppStartParams) -> Result<WebAppStatus> {
        if !request.listen.ip().is_loopback() {
            bail!("Runtime Web App only accepts a loopback address");
        }
        let language = Language::parse(Some(&request.language))?;
        request.workspace = request
            .workspace
            .canonicalize()
            .context("resolve Web App workspace")?;
        if !request.workspace.is_dir() {
            bail!("Web App workspace must be a directory");
        }
        let config_path = std::path::absolute(
            request
                .config
                .clone()
                .unwrap_or(crate::config::default_config_path()?),
        )?;
        request.config = Some(config_path.clone());
        let mut serving = self.serving.lock().await;
        if serving
            .as_ref()
            .is_some_and(|server| !server.task.is_finished())
        {
            let saved = load(&self.home)?.context("Web App settings missing")?;
            if saved.request != request {
                bail!("Web App is running with different settings; stop it before changing them");
            }
            return Ok(self.status());
        }
        if let Some(previous) = serving.take() {
            let _ = previous.task.await;
            let _ = previous.ready.await;
        }
        save(
            &self.home,
            &Settings {
                enabled: true,
                request: request.clone(),
            },
        )?;
        *self.status.lock().expect("Web App status") = WebAppStatus {
            enabled: true,
            running: false,
            listen: Some(request.listen),
            error: None,
        };
        let (stop, receiver) = watch::channel(false);
        let (started, ready) = tokio::sync::oneshot::channel();
        let status = self.status.clone();
        let config = crate::web::WebConfig {
            listen: request.listen,
            config_path,
            profile: request.profile.clone(),
            workspaces: vec![request.workspace.clone()],
            home: self.home.clone(),
            language,
            open_setup: false,
            setup_started: Some(started),
        };
        let task = tokio::spawn(async move {
            let result = crate::web::serve_managed(config, receiver).await;
            let mut status = status.lock().expect("Web App status");
            status.running = false;
            if let Err(error) = result {
                let occupied = error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::AddrInUse)
                });
                let message = if occupied {
                    language.text(
                        "Web 监听端口已被占用",
                        "Web listen address is already in use",
                        "Web の待受ポートは使用中です",
                    )
                } else {
                    language.text(
                        "Web 初始化或监听失败",
                        "Web initialization or listener failed",
                        "Web の初期化または待受に失敗しました",
                    )
                };
                eprintln!("Runtime Web App: {message}");
                status.error = Some(message.into());
            }
        });
        let status = self.status.clone();
        let home = self.home.clone();
        let (accepted, reply) = tokio::sync::oneshot::channel();
        // This observer belongs to Runtime too: cancelling the requesting
        // terminal must not lose readiness or the chosen port.
        let observer = tokio::spawn(async move {
            let result = match ready.await {
                Ok(address) => {
                    let mut status = status.lock().expect("Web App status");
                    if !status.enabled || status.error.is_some() {
                        Err("Web App stopped before readiness".to_owned())
                    } else {
                        request.listen = address;
                        match save(
                            &home,
                            &Settings {
                                enabled: true,
                                request,
                            },
                        ) {
                            Ok(()) => {
                                status.listen = Some(address);
                                status.running = true;
                                Ok(())
                            }
                            Err(_) => {
                                status.error = Some("cannot persist Web App address".into());
                                Err("cannot persist Web App address".into())
                            }
                        }
                    }
                }
                Err(_) => Err("Web App startup failed".to_owned()),
            };
            let _ = accepted.send(result);
        });
        *serving = Some(Serving {
            stop,
            task,
            ready: observer,
        });
        match tokio::time::timeout(Duration::from_secs(30), reply).await {
            Ok(Ok(Ok(()))) => Ok(self.status()),
            _ => {
                if let Some(server) = serving.take() {
                    server.ready.abort();
                    server.task.abort();
                    let _ = server.task.await;
                }
                let mut status = self.status.lock().expect("Web App status");
                if status.error.is_none() {
                    status.error = Some(
                        language
                            .text(
                                "Web 未能完成启动",
                                "Web App did not become ready",
                                "Web App を起動できませんでした",
                            )
                            .into(),
                    );
                }
                bail!(
                    "Web App did not become ready; check Runtime logs for port or initialization errors"
                )
            }
        }
    }

    pub(super) async fn stop(&self) -> Result<WebAppStatus> {
        let mut serving = self.serving.lock().await;
        {
            let mut status = self.status.lock().expect("Web App status");
            let was_enabled = status.enabled;
            if let Some(mut settings) = load(&self.home)? {
                settings.enabled = false;
                if let Err(error) = save(&self.home, &settings) {
                    status.enabled = was_enabled;
                    return Err(error);
                }
            }
            status.enabled = false;
        }
        if let Some(mut server) = serving.take() {
            server.ready.abort();
            let _ = server.stop.send(true);
            if tokio::time::timeout(Duration::from_secs(5), &mut server.task)
                .await
                .is_err()
            {
                server.task.abort();
                let _ = server.task.await;
            }
        }
        let mut status = self.status.lock().expect("Web App status");
        status.enabled = false;
        status.running = false;
        status.error = None;
        Ok(status.clone())
    }

    pub(super) fn resume(self: &Arc<Self>, shutdown: watch::Receiver<bool>) {
        let service = self.clone();
        tokio::spawn(async move {
            match load(&service.home) {
                Ok(Some(settings)) if settings.enabled => {
                    if let Err(error) = service.start(settings.request).await {
                        eprintln!("Runtime Web App not resumed: {error}");
                    }
                }
                Ok(_) => {}
                Err(error) => eprintln!("Runtime Web App settings unavailable: {error}"),
            }
            wait_for_shutdown(shutdown.clone()).await;
            // Runtime shutdown retains enabled intent. Tasks belong to this Runtime.
            if let Some(server) = service.serving.lock().await.take() {
                server.ready.abort();
                let _ = server.stop.send(true);
                server.task.abort();
            }
        });
    }
}

impl Drop for WebAppService {
    fn drop(&mut self) {
        if let Some(server) = self.serving.get_mut().take() {
            server.ready.abort();
            server.task.abort();
        }
    }
}

fn load(home: &Path) -> Result<Option<Settings>> {
    use std::io::Read;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(home.join("webapp-service.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        bail!("Web App settings must be a regular file");
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        bail!("Web App settings exceed size limit");
    }
    Ok(Some(
        serde_json::from_slice(&bytes).context("decode Web App settings")?,
    ))
}

fn save(home: &Path, settings: &Settings) -> Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(home)?;
    let temporary = home.join(format!(".{}.webapp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        let bytes = serde_json::to_vec(settings)?;
        if bytes.len() > 64 * 1024 {
            bail!("Web App settings exceed size limit");
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, home.join("webapp-service.json"))?;
        #[cfg(unix)]
        std::fs::File::open(home)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
