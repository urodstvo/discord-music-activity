//! Источник Windows Media Control (SMTC, GSMTC): локальный fallback,
//! когда Ynison недоступен или токен не задан (PLAN п.2).
//!
//! SMTC не даёт подписку, удобную для Rust-ядра, поэтому источник работает
//! polling'ом (~1 c): на каждом тике блокирующие COM-вызовы уходят в
//! `spawn_blocking` (helper `await_smtc` — в `super::helpers`).
//!
//! Отличия от Ynison, учтённые в нормализованной модели:
//! - `track_id` нет → обогащение по ID из `yandex::metadata` не нужно,
//!   mapper работает без `track_url` (кнопка не публикуется);
//! - обложки нет (thumbnail — локальный поток, сетевого HTTPS-URL не
//!   существует, а модель допускает только проверенные HTTPS-URL);
//! - `source_device` — app id медиа-сессии (напр. `Spotify.exe`); это
//!   основа выбора конкретного процесса (PLAN п.2.1): пустой
//!   `smtc_source_app` — авто (sticky + деприоритезация браузеров),
//!   заданный — строгая фиксация только на нём.
//!
//! Позиция трека: SMTC пишет `Position` только в момент обновления
//! timeline, поэтому актуальное значение = `Position + (now −
//! LastUpdatedTime)` при Playing (значение ОС честно «стареет» с момента
//! установки). Это даёт монотонный таймстамп старта трека в Discord без
//! осцилляций.
//!
//! Стабильность против глитчей ОС:
//! - пустой список сессий / ошибка опроса НЕ сбрасывают выбор сразу:
//!   очистка публикуется только после серии подряд идущих промахов
//!   (`EMPTY_MISS_LIMIT`), одиночный глитч (сворачивание окна плеера,
//!   переход трека) выбор и таймер не роняет;
//! - seek/позиционный скачок при том же треке публикуется только после
//!   подтверждения следующим тиком — одиночный кадр с `Position=0`
//!   (буферизация, переход) таймер Discord не сбрасывает;
//! - смена трека/статуса публикуется сразу.
//!
//! Источник НЕ пишет статусы в snapshot (это делает Supervisor), он только
//! шлёт `SourceEvent` в общий канал. Закрепление читается из настроек на
//! каждом тике — перезапуск источника при смене не нужен.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

use crate::app_state::AppSharedState;
use crate::sources::smtc::helpers::await_smtc;
use crate::sources::{PlaybackState, PlaybackStatus, SourceEvent, SourceKind};

/// Период опроса SMTC.
pub const POLL_INTERVAL: Duration = Duration::from_millis(1_000);
/// Общий срок на один опрос (COM-цепочка: менеджер → сессии → свойства).
const POLL_TOTAL_TIMEOUT_MS: u64 = 2_000;
/// Повторные ошибки опроса подряд, после которых считаем данные потерянными.
const ERROR_RETRY_LIMIT: u32 = 3;
/// Подряд идущие пустые результаты (сессий нет / закреплённого нет),
/// после которых публикуем очистку. Одиночные глитчи выбор не роняют.
const EMPTY_MISS_LIMIT: u32 = 3;
/// Смещение позиции, значимое для публикации (фронтенд-часы сами
/// экстраполируют прогресс между событиями — избыточный спам UI не нужен).
const EMIT_POSITION_DELTA_MS: u64 = 5_000;
/// Допуск подтверждения seek вторым тиком: позиция-кандидат считается
/// подтверждённой, если следующий замер в пределах ±3 c от неё.
const SEEK_CONFIRM_TOLERANCE_MS: u64 = 3_000;
/// Допуск «отката к опубликованному»: замер в пределах ±3 c от уже
/// опубликованной позиции — не seek, а transient-глюк/лаг плеера
/// (прыгнул и вернулся, буферизация промотки). Discord это уже
/// показывает — молчим и гасим кандидата.
const REVERT_TOLERANCE_MS: u64 = 3_000;
/// Сколько подряд тиков с неизменной сырой позицией при Playing считаем
/// timeline мёртвым: плеер не двигает `Position` (напр. Yandex Desktop
/// отдаёт константу ~334 мс весь трек при свежем `LastUpdatedTime`).
/// Пока мёртв — позиционные скачки не публикуем: фронт и Discord
/// экстраполируют время сами от последнего события. Иначе детектор
/// «необъяснимого прогресса» видел бы стоящую позицию при растущем
/// ожидании и сбрасывал таймер перепубликацией каждые ~6 c.
const FROZEN_STABLE_POLLS: u32 = 3;
/// Разница эпох между filetime (1601-01-01) и Unix (1970-01-01), в 100-нс тиках.
const UNIX_TO_FILETIME_TICKS: i64 = 11_644_473_600_000_000;
/// Возраст позиции больше суток — заведомо мусорная метка ОС
/// (см. `map_state`).
const MAX_PLAUSIBLE_AGE_MS: u64 = 86_400_000;

/// Короткое описание видимой медиа-сессии для UI выбора процесса
/// (команда `list_media_sessions`, секретов нет — только отображаемое).
/// Позиционные поля нужны для диагностики рассинхрона времени:
/// пользователь видит сырые числа ОС рядом с реальным плеером.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionInfo {
    pub app_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub status: PlaybackStatus,
    /// Сырая позиция ОС на момент опроса, мс.
    pub position_raw_ms: u64,
    /// Возраст сырого значения (now − LastUpdatedTime), мс.
    pub position_age_ms: u64,
    /// Итоговая позиция с экстраполяцией (та же, что уходит в Presence).
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// ОС считает эту сессию текущей (`GetCurrentSession`).
    pub is_current: bool,
}

/// Синхронный снимок видимых сессий (для команды листинга).
/// Вызывать только из `spawn_blocking`: внутри блокирующие COM-вызовы.
pub fn snapshot_sessions_blocking() -> Vec<MediaSessionInfo> {
    match poll_smtc() {
        Ok(snap) => {
            let current = snap.current_app_id.clone();
            snap.sessions
                .into_iter()
                .map(|raw| {
                    let is_current = current.as_deref().is_some_and(|cur| cur == raw.app_id);
                    let computed = map_state(&raw);
                    MediaSessionInfo {
                        app_id: raw.app_id,
                        title: raw.title,
                        artists: raw.artists,
                        status: raw.status,
                        position_raw_ms: raw.position_ms,
                        position_age_ms: raw.position_age_ms,
                        position_ms: computed.position_ms,
                        duration_ms: raw.duration_ms,
                        is_current,
                    }
                })
                .collect()
        }
        Err(err) => {
            tracing::debug!("list_media_sessions poll failed: {err}");
            Vec::new()
        }
    }
}

/// Запустить SMTC-источник как фоновую задачу.
pub fn spawn_smtc_source(
    shutdown: CancellationToken,
    events: mpsc::UnboundedSender<SourceEvent>,
    shared: Arc<AppSharedState>,
) {
    tauri::async_runtime::spawn(async move {
        tracing::info!("SMTC источник запущен");
        let mut sticky_app: Option<String> = None;
        let mut emitted: Option<EmitMemo> = None;
        // Кандидат на seek: позиционный скачок публикуется только после
        // подтверждения следующим тиком (одиночный кадр с Position=0
        // при буферизации/переходе таймер не сбрасывает).
        let mut seek_candidate: Option<EmitMemo> = None;
        let mut errors = 0u32;
        let mut empty_misses = 0u32;
        // Детект мёртвого timeline: сырая позиция не двигается тик за тиком.
        let mut last_raw_pos: Option<u64> = None;
        let mut stable_raw_count: u32 = 0;

        loop {
            let tick = tokio::time::sleep(POLL_INTERVAL);
            tokio::select! {
                _ = shutdown.cancelled() => {
                    let _ = events.send(SourceEvent::Stopped { kind: SourceKind::MediaControl });
                    tracing::info!("SMTC источник остановлен");
                    return;
                }
                _ = tick => {}
            }

            // Закрепление читаем на каждом тике: смена настройки
            // применяется без перезапуска источника.
            let pinned = {
                let app = shared.settings().smtc_source_app;
                let trimmed = app.trim().to_string();
                (!trimmed.is_empty()).then_some(trimmed)
            };

            let outcome = tokio::time::timeout(
                Duration::from_millis(POLL_TOTAL_TIMEOUT_MS),
                tokio::task::spawn_blocking(poll_smtc),
            )
            .await;

            match outcome {
                // Транзиентная ошибка: держим последнее состояние, после
                // серии ошибок объявляем потерю данных (координатор очистит
                // Activity по grace-политике).
                Ok(Err(join)) => {
                    errors += 1;
                    tracing::debug!("SMTC poll join failed: {join:?}");
                }
                Ok(Ok(Err(err))) => {
                    errors += 1;
                    tracing::debug!("SMTC poll failed: {err}");
                }
                Err(_) => {
                    errors += 1;
                    tracing::debug!("SMTC poll timed out ({POLL_TOTAL_TIMEOUT_MS} мс)");
                }
                Ok(Ok(Ok(snap))) => {
                    let now = Instant::now();
                    errors = 0;
                    let current = snap.current_app_id.clone();

                    // Строгая фиксация: чужие процессы не рассматриваются
                    // вообще, пока закрепление задано (PLAN п.2.1).
                    let eligible: Vec<&RawSession> = match pinned.as_deref() {
                        Some(pin) => snap
                            .sessions
                            .iter()
                            .filter(|s| s.app_id.eq_ignore_ascii_case(pin))
                            .collect(),
                        None => snap.sessions.iter().collect(),
                    };
                    // Смена режима авто ↔ фиксация сбрасывает старое
                    // прилипание: чужой sticky не должен переживать pin.
                    if let Some(pin) = pinned.as_deref() {
                        if sticky_app.as_deref() != Some(pin) {
                            sticky_app = None;
                        }
                    }
                    let owned: Vec<RawSession> = eligible.into_iter().cloned().collect();

                    match select_session(&owned, sticky_app.as_deref(), current.as_deref()) {
                        // Сессий нет: НЕ сбрасываем выбор сразу — ждём
                        // серию промахов (сворачивание плеера, переход
                        // трека и прочие глитчи ОС дают одиночные пустые
                        // тики; мгновенная очистка роняла бы таймер).
                        None => {
                            empty_misses += 1;
                            seek_candidate = None;
                            if empty_misses >= EMPTY_MISS_LIMIT {
                                sticky_app = None;
                                if emitted.is_some() {
                                    emitted = None;
                                    let _ = events.send(SourceEvent::State {
                                        kind: SourceKind::MediaControl,
                                        state: None,
                                    });
                                }
                            }
                        }
                        Some(raw) => {
                            empty_misses = 0;
                            // Прилипание по app id: смена трека внутри того
                            // же приложения не перекидывает выбор на другой
                            // плеер (иначе таймстампы в Discord сбрасываются).
                            if !raw.app_id.is_empty() && sticky_app.as_deref() != Some(&raw.app_id)
                            {
                                sticky_app = Some(raw.app_id.clone());
                                tracing::debug!("SMTC сессия прилипла: {}", raw.app_id);
                            }
                            // Мёртвый timeline: raw стоит на месте тик за
                            // тиком при Playing — плеер позицию не ведёт.
                            if Some(raw.position_ms) == last_raw_pos {
                                stable_raw_count = stable_raw_count.saturating_add(1);
                            } else {
                                last_raw_pos = Some(raw.position_ms);
                                stable_raw_count = 0;
                            }
                            let frozen = is_frozen_timeline(stable_raw_count, raw.status);
                            let state = map_state(raw);
                            tracing::debug!(
                                "SMTC {:?} {:?}: raw={}мс age={}мс computed={:?}мс dur={:?}мс{}",
                                raw.app_id,
                                raw.status,
                                raw.position_ms,
                                raw.position_age_ms,
                                state.position_ms,
                                state.duration_ms,
                                if frozen { " FROZEN" } else { "" }
                            );
                            match classify_change(emitted.as_ref(), &state, now) {
                                Change::Same => {
                                    seek_candidate = None;
                                }
                                Change::Identity => {
                                    seek_candidate = None;
                                    emitted = Some(EmitMemo::snapshot(&state, now));
                                    let _ = events.send(SourceEvent::State {
                                        kind: SourceKind::MediaControl,
                                        state: Some(state),
                                    });
                                }
                                Change::Seek if frozen || is_revert(emitted.as_ref(), &state) => {
                                    // Стоящая позиция при растущем ожидании —
                                    // не seek, а мёртвый timeline; откат к
                                    // уже опубликованному — не seek, а лаг
                                    // плеера. Молчим: фронт и Discord
                                    // досчитают сами.
                                    seek_candidate = None;
                                }
                                Change::Seek => {
                                    // Требуем подтверждения: следующий тик
                                    // должен показать позицию рядом с
                                    // кандидатом (тот же трек/статус).
                                    let confirmed = seek_candidate.as_ref().is_some_and(|cand| {
                                        cand.title == state.title
                                            && cand.status == state.status
                                            && cand.artists == state.artists
                                            && state
                                                .position_ms
                                                .unwrap_or(0)
                                                .abs_diff(cand.position_ms)
                                                <= SEEK_CONFIRM_TOLERANCE_MS
                                    });
                                    if confirmed {
                                        seek_candidate = None;
                                        emitted = Some(EmitMemo::snapshot(&state, now));
                                        let _ = events.send(SourceEvent::State {
                                            kind: SourceKind::MediaControl,
                                            state: Some(state),
                                        });
                                    } else {
                                        seek_candidate = Some(EmitMemo::snapshot(&state, now));
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
            }

            // Сюда попадаем только на ошибку опроса.
            if errors >= ERROR_RETRY_LIMIT && emitted.is_some() {
                emitted = None;
                seek_candidate = None;
                empty_misses = 0;
                let _ = events.send(SourceEvent::State {
                    kind: SourceKind::MediaControl,
                    state: None,
                });
            }
        }
    });
}

/// Откат ли это к уже опубликованной позиции (transient-глюк/лаг
/// плеера при промотке, а не настоящий seek). Статус/трек/альбом при этом
/// совпадают — это гарантирует вызывающий код (ветка `Change::Seek`).
fn is_revert(memo: Option<&EmitMemo>, next: &PlaybackState) -> bool {
    let Some(memo) = memo else {
        return false;
    };
    next.position_ms.unwrap_or(0).abs_diff(memo.position_ms) <= REVERT_TOLERANCE_MS
}

/// Мёртвый ли timeline: сырая позиция не меняется подряд тиков при
/// Playing/Unknown. Пауза/стоп сюда не попадают: там стоящая позиция —
/// норма, а переходы статусов публикуются как Identity в любом случае.
fn is_frozen_timeline(stable_polls: u32, status: PlaybackStatus) -> bool {
    matches!(status, PlaybackStatus::Playing | PlaybackStatus::Unknown)
        && stable_polls >= FROZEN_STABLE_POLLS
}

/// Характер отличия нового состояния от опубликованного.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Ничего значимого: естественный прогресс в пределах допуска.
    Same,
    /// Смена трека/статуса/альбома/длительности — публикуем сразу.
    Identity,
    /// Скачок позиции при том же треке — ждём подтверждения.
    Seek,
}

/// Классификация отличия: сначала идентичность (трек/статус), затем
/// объяснимость прогресса позиции.
fn classify_change(prev: Option<&EmitMemo>, next: &PlaybackState, now: Instant) -> Change {
    let Some(prev) = prev else {
        return Change::Identity;
    };
    if prev.title != next.title
        || prev.status != next.status
        || prev.artists != next.artists
        || prev.album != next.album
        || prev.duration_ms != next.duration_ms
    {
        return Change::Identity;
    }
    // Проверяем «объяснимость» прогресса: позиция могла расти ровно на
    // время между публикациями — такой тик публиковать не нужно.
    let position = next.position_ms.unwrap_or(0);
    let expected = now.saturating_duration_since(prev.at).as_millis() as u64;
    if position.abs_diff(prev.position_ms).abs_diff(expected) > EMIT_POSITION_DELTA_MS {
        Change::Seek
    } else {
        Change::Same
    }
}

/// Что публикует источник: идентичность трека/статуса — всегда сразу;
/// сдвиг позиции при том же треке — только после подтверждения.
#[derive(Debug, Clone)]
struct EmitMemo {
    title: Option<String>,
    artists: Vec<String>,
    album: Option<String>,
    duration_ms: Option<u64>,
    status: PlaybackStatus,
    position_ms: u64,
    at: Instant,
}

impl EmitMemo {
    fn snapshot(state: &PlaybackState, now: Instant) -> Self {
        Self {
            title: state.title.clone(),
            artists: state.artists.clone(),
            album: state.album.clone(),
            duration_ms: state.duration_ms,
            status: state.status,
            position_ms: state.position_ms.unwrap_or(0),
            at: now,
        }
    }
}

/// Ключевые поля одной SMTC-сессии (после COM-агрегации).
#[derive(Debug, Clone)]
pub struct RawSession {
    pub app_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    /// Позиция, зафиксированная ОС в момент `LastUpdatedTime`.
    pub position_ms: u64,
    /// Сколько мс прошло с момента установки позиции ОС (`LastUpdatedTime`).
    /// 0 — время недоступно (считаем позицию свежей).
    pub position_age_ms: u64,
    pub duration_ms: Option<u64>,
    pub status: PlaybackStatus,
}

/// Нормализованное состояние из сессии. Экстраполяция позиции честная:
/// при Playing ОС-значение «стареет» на возраст `LastUpdatedTime`, при
/// паузе — заморожено как есть.
fn map_state(raw: &RawSession) -> PlaybackState {
    // Мусорная метка времени ОС (плеер не ставит LastUpdatedTime — там
    // 1601 год или заморозка с прошлого трека) иначе катапультировала бы
    // позицию сразу в конец трека. Возраст больше суток заведомо
    // невозможен для живой позиции — считаем значение свежим.
    let age_ms = if raw.position_age_ms > MAX_PLAUSIBLE_AGE_MS {
        tracing::debug!(
            "SMTC {:?}: неправдоподобный возраст позиции {} мс — игнорирую",
            raw.app_id,
            raw.position_age_ms
        );
        0
    } else {
        raw.position_age_ms
    };
    let mut position_ms = match raw.status {
        PlaybackStatus::Playing => raw.position_ms.saturating_add(age_ms),
        _ => raw.position_ms,
    };
    // Страховка от «убежавшей» позиции плеера, который перестал обновлять
    // timeline, но остался в статусе Playing.
    if let Some(duration) = raw.duration_ms {
        if position_ms > duration {
            position_ms = duration;
        }
    }

    PlaybackState {
        status: raw.status,
        track_id: None,
        title: Some(raw.title.clone()),
        artists: raw.artists.clone(),
        album: (!raw.album.is_empty()).then(|| raw.album.clone()),
        duration_ms: raw.duration_ms,
        position_ms: Some(position_ms),
        cover_url: None,
        track_url: None,
        source_device: Some(raw.app_id.clone()),
        observed_at: SystemTime::now(),
    }
}

/// Известные фрагменты app id браузеров: их медиа-сессии депrioritизируются
/// против десктоп-приложений (типовой кейс: фоновое видео на YouTube-вкладке
/// перехватывало Presence у Яндекс Музыки).
const BROWSER_APP_FRAGMENTS: [&str; 11] = [
    "chrome", "chromium", "msedge", "edge", "firefox", "opera", "brave", "vivaldi", "arc",
    "safari", "browser",
];

/// Похоже ли приложение на браузер (эвристика по app id).
fn is_browser(app_id: &str) -> bool {
    let id = app_id.to_lowercase();
    BROWSER_APP_FRAGMENTS.iter().any(|frag| id.contains(frag))
}

/// Выбор сессии для публикации.
///
/// 1. Sticky: продолжаем слушать то же приложение (по app id), пока его
///    сессия жива, — статус/трек/пауза не меняют выбор.
/// 2. Иначе приоритет: не-браузер + Playing + текущая по ОС >
///    не-браузер + Playing > остальное рангами (браузер ниже,
///    не-играющее ниже, не-текущее ниже); внутри одного ранга — порядок
///    списка ОС (стабильная сортировка). Текущая сессия
///    (`GetCurrentSession`, как в WinYandexMusicRPC) спасает от протухших
///    заглушек того же плеера.
fn select_session<'a>(
    sessions: &'a [RawSession],
    sticky: Option<&str>,
    current: Option<&str>,
) -> Option<&'a RawSession> {
    if let Some(app) = sticky {
        if let Some(found) = sessions
            .iter()
            .find(|s| !s.app_id.is_empty() && s.app_id == app)
        {
            return Some(found);
        }
    }
    sessions.iter().min_by_key(|s| {
        (
            u8::from(is_browser(&s.app_id)),
            u8::from(s.status != PlaybackStatus::Playing),
            u8::from(current != Some(s.app_id.as_str())),
        )
    })
}

/// Результат одного опроса SMTC: сессии + «текущая» сессия по мнению ОС.
/// `GetCurrentSession` — то, что WinYandexMusicRPC ставит в приоритет:
/// при нескольких сессиях (в т.ч. протухших заглушек того же приложения)
/// ОС обычно указывает на реально активную.
pub struct SmtcSnapshot {
    pub sessions: Vec<RawSession>,
    pub current_app_id: Option<String>,
}

/// Блокирующий опрос SMTC (вызывается через spawn_blocking).
/// Возвращает все читаемые сессии; выбор — отдельно (`select_session`).
fn poll_smtc() -> Result<SmtcSnapshot, windows::core::Error> {
    let deadline = Instant::now() + Duration::from_millis(POLL_TOTAL_TIMEOUT_MS);

    let manager = await_smtc(
        GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?,
        deadline,
    )?;
    // Текущая сессия может отсутствовать (ничего не играет) — это норма.
    let current_app_id = manager
        .GetCurrentSession()
        .ok()
        .and_then(|s| s.SourceAppUserModelId().ok())
        .map(|v| v.to_string());
    let sessions = manager.GetSessions()?;
    let mut out = Vec::new();
    let count = sessions.Size()?;
    for i in 0..count {
        let session = sessions.GetAt(i)?;
        if let Ok(Some(info)) = read_session(&session, deadline) {
            out.push(info);
        }
    }
    Ok(SmtcSnapshot {
        sessions: out,
        current_app_id,
    })
}

/// Прочитать одну сессию: без паник на пустых/ошибочных полях.
/// `None` — сессия непригодна (пустой заголовок).
fn read_session(
    session: &windows::Media::Control::GlobalSystemMediaTransportControlsSession,
    deadline: Instant,
) -> Result<Option<RawSession>, windows::core::Error> {
    let app_id = session.SourceAppUserModelId().map(|v| v.to_string());

    let props = await_smtc(session.TryGetMediaPropertiesAsync()?, deadline)?;
    let title = props.Title().map(|v| v.to_string()).unwrap_or_default();
    if title.trim().is_empty() {
        // Пустой заголовок бесполезен для Presence.
        return Ok(None);
    }
    let artists = props
        .Artist()
        .map(|a| split_artists(&a.to_string()))
        .unwrap_or_default();
    let album = props
        .AlbumTitle()
        .map(|v| v.to_string())
        .unwrap_or_default();

    let timeline = session.GetTimelineProperties()?;
    // Позиция timeline — смещение от StartTime (как в WinYandexMusicRPC:
    // `position − start`, иначе у плееров с ненулевым стартом шкала съедет).
    let start_ms = timeline.StartTime().ok().and_then(timespan_ms).unwrap_or(0);
    let position_ms = timeline
        .Position()
        .ok()
        .and_then(timespan_ms)
        .map(|pos| pos.saturating_sub(start_ms))
        .unwrap_or(0);
    // Возраст ОС-значения позиции: сейчас (filetime) − LastUpdatedTime.
    // Отрицательный возраст (расхождение часов) трактуем как 0.
    let position_age_ms = timeline
        .LastUpdatedTime()
        .ok()
        .map(|dt| {
            let now_unix_nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            let now_ticks = now_unix_nanos / 100 + UNIX_TO_FILETIME_TICKS;
            let age_ms = (now_ticks - dt.UniversalTime) / 10_000;
            age_ms.clamp(0, i64::from(u32::MAX)) as u64
        })
        .unwrap_or(0);
    // Длительность трека: EndTime − StartTime (MaxSeekTime — про seek-грань).
    let duration_ms = {
        let end = timeline.EndTime().ok().and_then(timespan_ms);
        let start = timeline.StartTime().ok().and_then(timespan_ms);
        match (end, start) {
            (Some(end), Some(start)) if end > start => Some(end - start),
            (Some(end), Some(_)) | (Some(end), None) => Some(end),
            _ => None,
        }
    };
    let status = session
        .GetPlaybackInfo()
        .ok()
        .and_then(|info| info.PlaybackStatus().ok())
        .map(map_status)
        .unwrap_or(PlaybackStatus::Unknown);

    Ok(Some(RawSession {
        app_id: app_id.unwrap_or_default(),
        title: title.trim().to_string(),
        artists,
        album,
        position_ms,
        position_age_ms,
        duration_ms,
        status,
    }))
}

/// Строка `Artist1, Artist2` → список исполнителей без пустых.
/// Расщепляем по любой запятой (SMTC-игроки используют `", "` и `","`).
fn split_artists(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// TimeSpan (100-нс тики) в миллисекунды; неположительные → None.
fn timespan_ms(ts: windows::Foundation::TimeSpan) -> Option<u64> {
    if ts.Duration <= 0 {
        return None;
    }
    Some((ts.Duration as u64) / 10_000)
}

/// SMTC-статус → нормализованный статус.
/// `Opened` не даёт достоверности о воспроизведении.
fn map_status(status: GlobalSystemMediaTransportControlsSessionPlaybackStatus) -> PlaybackStatus {
    use GlobalSystemMediaTransportControlsSessionPlaybackStatus as Raw;
    match status {
        Raw::Playing => PlaybackStatus::Playing,
        Raw::Paused => PlaybackStatus::Paused,
        // Closed/Stopped — явная остановка; Opened — трек загружен,
        // но про воспроизведение сказать нечего.
        Raw::Closed | Raw::Stopped => PlaybackStatus::Stopped,
        Raw::Opened => PlaybackStatus::Unknown,
        _ => PlaybackStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(app_id: &str, status: PlaybackStatus, title: &str, position: u64) -> RawSession {
        RawSession {
            app_id: app_id.into(),
            title: title.into(),
            artists: vec!["A".into()],
            album: String::new(),
            position_ms: position,
            position_age_ms: 0,
            duration_ms: Some(200_000),
            status,
        }
    }

    fn state(status: PlaybackStatus, title: &str, position: u64) -> PlaybackState {
        PlaybackState {
            status,
            track_id: None,
            title: Some(title.into()),
            artists: vec!["A".into()],
            album: None,
            duration_ms: Some(200_000),
            position_ms: Some(position),
            cover_url: None,
            track_url: None,
            source_device: None,
            observed_at: SystemTime::now(),
        }
    }

    #[test]
    fn split_artists_splits_on_comma_and_trims() {
        assert_eq!(
            split_artists("Океан Ельзи, The Beatles ,  KAZKA"),
            vec!["Океан Ельзи", "The Beatles", "KAZKA"]
        );
        assert!(split_artists("").is_empty());
        assert!(split_artists(",,").is_empty());
    }

    #[test]
    fn timespan_conversion() {
        // 3 секунды = 30_000_000 тиков (по 100 нс).
        assert_eq!(
            timespan_ms(windows::Foundation::TimeSpan {
                Duration: 30_000_000
            }),
            Some(3_000)
        );
        assert_eq!(
            timespan_ms(windows::Foundation::TimeSpan { Duration: 0 }),
            None
        );
        assert_eq!(
            timespan_ms(windows::Foundation::TimeSpan { Duration: -1 }),
            None
        );
    }

    #[test]
    fn first_state_always_published() {
        assert_eq!(
            classify_change(
                None,
                &state(PlaybackStatus::Playing, "Track", 0),
                Instant::now()
            ),
            Change::Identity
        );
    }

    #[test]
    fn same_track_progress_not_published() {
        let now = Instant::now();
        let prev = EmitMemo::snapshot(
            &state(PlaybackStatus::Playing, "Track", 0),
            now - Duration::from_millis(3_000),
        );
        let next = state(PlaybackStatus::Playing, "Track", 3_000);
        assert_eq!(
            classify_change(Some(&prev), &next, now),
            Change::Same,
            "ожидаемый прогресс не публикуем"
        );
    }

    #[test]
    fn status_change_is_identity() {
        let now = Instant::now();
        let prev = EmitMemo::snapshot(&state(PlaybackStatus::Playing, "Track", 1_000), now);
        let next = state(PlaybackStatus::Paused, "Track", 1_000);
        assert_eq!(classify_change(Some(&prev), &next, now), Change::Identity);
    }

    #[test]
    fn unexplained_seek_needs_confirm() {
        let now = Instant::now();
        let prev = EmitMemo::snapshot(
            &state(PlaybackStatus::Playing, "Track", 0),
            now - Duration::from_millis(1_000),
        );
        let next = state(PlaybackStatus::Playing, "Track", 60_000);
        assert_eq!(
            classify_change(Some(&prev), &next, now),
            Change::Seek,
            "seek виден, но публикуется после подтверждения"
        );
    }

    #[test]
    fn artist_change_is_identity_not_seek() {
        // Тот же заголовок, но другой исполнитель — смена трека
        // (радио/плейлист с повтором названия), публикуем сразу.
        let now = Instant::now();
        let mut next = state(PlaybackStatus::Playing, "Track", 3_000);
        next.artists = vec!["B".into()];
        let prev = EmitMemo::snapshot(
            &state(PlaybackStatus::Playing, "Track", 0),
            now - Duration::from_millis(3_000),
        );
        assert_eq!(classify_change(Some(&prev), &next, now), Change::Identity);
    }

    #[test]
    fn revert_to_published_is_not_a_seek() {
        // Лаг промотки: прыгнул на 100 c и вернулся на 40 c (уже
        // опубликованы) — это не seek, публиковать нечего.
        let now = Instant::now();
        let memo = EmitMemo::snapshot(&state(PlaybackStatus::Playing, "Track", 40_000), now);
        let back = state(PlaybackStatus::Playing, "Track", 40_500);
        assert!(is_revert(Some(&memo), &back));
        // Настоящий seek мимо опубликованного — не откат.
        let forward = state(PlaybackStatus::Playing, "Track", 100_000);
        assert!(!is_revert(Some(&memo), &forward));
        assert!(!is_revert(None, &forward));
    }

    #[test]
    fn frozen_timeline_suppresses_only_seek() {
        // 3+ тика с тем же raw при Playing — мёртвый timeline.
        assert!(is_frozen_timeline(3, PlaybackStatus::Playing));
        assert!(is_frozen_timeline(10, PlaybackStatus::Playing));
        // Меньше порога, пауза или стоп — не мёртвый.
        assert!(!is_frozen_timeline(2, PlaybackStatus::Playing));
        assert!(!is_frozen_timeline(0, PlaybackStatus::Playing));
        assert!(!is_frozen_timeline(10, PlaybackStatus::Paused));
        assert!(!is_frozen_timeline(10, PlaybackStatus::Stopped));
        // Unknown — тот же мёртвый timeline, что и Playing.
        assert!(is_frozen_timeline(3, PlaybackStatus::Unknown));
    }

    #[test]
    fn frozen_raw_reports_seek_but_loop_downgrades_it() {
        // Кейс из лога: raw=334 стоит весь трек. Ожидание растёт,
        // classify видит Seek — цикл даунгрейдит его из-за frozen.
        let now = Instant::now();
        let at = now - Duration::from_millis(6_000);
        let prev = EmitMemo::snapshot(&state(PlaybackStatus::Playing, "Track", 334), at);
        let next = state(PlaybackStatus::Playing, "Track", 334);
        assert_eq!(classify_change(Some(&prev), &next, now), Change::Seek);
        assert!(
            is_frozen_timeline(3, next.status),
            "при стоящем raw Seek должен гаситься вызывающим кодом"
        );
    }

    #[test]
    fn map_state_publishes_without_track_id() {
        let state = map_state(&raw("App.X", PlaybackStatus::Playing, "Track", 10_000));
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.position_ms, Some(10_000));
        assert_eq!(state.source_device.as_deref(), Some("App.X"));
        assert!(state.track_id.is_none(), "SMTC не даёт track_id");
        assert!(state.cover_url.is_none(), "SMTC не даёт сетевой обложки");
        assert!(state.track_url.is_none(), "кнопки по локальному треку нет");
    }

    #[test]
    fn map_state_advances_playing_position_by_age() {
        let mut session = raw("App.X", PlaybackStatus::Playing, "Track", 10_000);
        session.position_age_ms = 4_000;
        let state = map_state(&session);
        assert_eq!(
            state.position_ms,
            Some(14_000),
            "при Playing позиция ОС стареет на возраст LastUpdatedTime"
        );
    }

    #[test]
    fn map_state_freezes_position_on_pause() {
        let mut session = raw("App.X", PlaybackStatus::Paused, "Track", 10_000);
        session.position_age_ms = 7_000;
        let state = map_state(&session);
        assert_eq!(
            state.position_ms,
            Some(10_000),
            "пауза не продвигается независимо от возраста"
        );
    }

    #[test]
    fn map_state_ignores_absurd_age() {
        // Плеер не ставит LastUpdatedTime (там 1601 год): без защиты
        // позиция катапультировалась бы в конец трека.
        let mut session = raw("App.X", PlaybackStatus::Playing, "Track", 10_000);
        session.position_age_ms = u64::from(u32::MAX) * 1000;
        let state = map_state(&session);
        assert_eq!(state.position_ms, Some(10_000));
    }

    #[test]
    fn map_state_clamps_runaway_position_to_duration() {
        let mut session = raw("App.X", PlaybackStatus::Playing, "Track", 199_000);
        session.position_age_ms = 10_000;
        let state = map_state(&session);
        assert_eq!(
            state.position_ms,
            Some(200_000),
            "позиция не убегает за длительность трека"
        );
    }

    #[test]
    fn select_prefers_desktop_app_over_browser() {
        let browser = vec![raw("Chrome.exe", PlaybackStatus::Playing, "Video", 5_000)];
        assert_eq!(
            select_session(&browser, None, None).map(|s| s.app_id.as_str()),
            Some("Chrome.exe"),
            "если есть только браузер — берём его"
        );

        let both = vec![
            raw("Chrome.exe", PlaybackStatus::Playing, "Video", 5_000),
            raw("YandexMusic.exe", PlaybackStatus::Playing, "Track", 1_000),
        ];
        assert_eq!(
            select_session(&both, None, None).map(|s| s.app_id.as_str()),
            Some("YandexMusic.exe"),
            "десктоп-приложение перебивает играющий браузер"
        );

        let app_paused = vec![
            raw("Chrome.exe", PlaybackStatus::Playing, "Video", 5_000),
            raw("YandexMusic.exe", PlaybackStatus::Paused, "Track", 1_000),
        ];
        assert_eq!(
            select_session(&app_paused, None, None).map(|s| s.app_id.as_str()),
            Some("YandexMusic.exe"),
            "даже пауза приложения важнее играющего браузера"
        );
    }

    #[test]
    fn select_is_sticky_per_app() {
        // Sticky держится, даже если приложение стало паузой и появился
        // другой играющий плеер — иначе таймстампы перекосятся.
        let sessions = vec![
            raw("Other.exe", PlaybackStatus::Playing, "Other", 0),
            raw("YandexMusic.exe", PlaybackStatus::Paused, "Track", 1_000),
        ];
        assert_eq!(
            select_session(&sessions, Some("YandexMusic.exe"), None).map(|s| s.app_id.as_str()),
            Some("YandexMusic.exe")
        );

        // Sticky умирает вместе с сессией.
        let gone = vec![raw("Other.exe", PlaybackStatus::Playing, "Other", 0)];
        assert_eq!(
            select_session(&gone, Some("YandexMusic.exe"), None).map(|s| s.app_id.as_str()),
            Some("Other.exe")
        );
    }

    #[test]
    fn select_without_sessions_is_none() {
        assert!(select_session(&[], None, None).is_none());
    }

    #[test]
    fn select_prefers_os_current_session() {
        // Две живые сессии одного ранга: ОС указывает на вторую
        // (протухшая заглушка первой в списке игнорируется).
        let sessions = vec![
            raw("AppA.exe", PlaybackStatus::Playing, "Stale", 1_000),
            raw("AppB.exe", PlaybackStatus::Playing, "Live", 2_000),
        ];
        assert_eq!(
            select_session(&sessions, None, Some("AppB.exe")).map(|s| s.title.as_str()),
            Some("Live")
        );
        // Без подсказки ОС — порядок списка.
        assert_eq!(
            select_session(&sessions, None, None).map(|s| s.title.as_str()),
            Some("Stale")
        );
    }

    #[test]
    fn browser_detection() {
        assert!(is_browser("Chrome.exe"));
        assert!(is_browser("Microsoft.MicrosoftEdge_8wekyb3d8bbwe!App"));
        assert!(is_browser("Mozilla.Firefox-308046-B0BA709E"));
        assert!(is_browser("YandexBrowser.exe"));
        assert!(!is_browser("YandexMusic.exe"));
        assert!(!is_browser("Spotify.exe"));
        assert!(!is_browser(""));
    }
}
