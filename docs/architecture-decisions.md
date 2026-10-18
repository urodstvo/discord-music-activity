# Архитектурные решения

## ADR-001: Базовый стек

- **Статус:** принято
- **Дата:** 2026-10-16

Для приложения выбран Tauri 2 с backend-ядром на Rust и frontend на Vue 3,
TypeScript и Vite. Node.js используется только для разработки и сборки
frontend; production-процесс не содержит Node.js sidecar.

## ADR-002: Границы первого этапа

Сгенерированный Tauri/Vue каркас является стартовой точкой. До реализации UI
должны быть исследованы и задокументированы отдельно:

- реальный handshake и формат событий Ynison;
- Discord IPC, публикация `LISTENING` activity и её очистка;
- поддержка динамической обложки и безопасный fallback;
- лицензии и совместимость выбранных Rust-зависимостей.

Ни один mock не будет использоваться как production-источник playback.

## ADR-003: Секреты

OAuth-токен Яндекс Музыки не будет храниться в файлах конфигурации, frontend
storage, логах или аргументах процессов. На Windows он будет сохраняться через
Windows Credential Manager из Rust-слоя; конкретный crate и его настройка будут
зафиксированы после technical spike.

## ADR-004: Реализация Ynison-клиента

- **Статус:** принято
- **Дата:** 2026-10-16

По результатам `docs/research/ynison-protocol.md`:

- Подключение двухшаговое: `GetRedirectToYnison` → state-сокет c
  subprotocols `Bearer`, `v2`, percent-encoded JSON метаданными устройства.
- Кадры сервера — полные снапшоты; толерантный разбор, позиция считается от
  `status.version.timestampMs` с учётом `playbackSpeed` (escrow-заморозка при
  паузе, игнор «протухших» таймштампов > 24 ч).
- Keep-alive — протокольные WS ping/pong по интервалу из `keepAliveParams`.
- Device id — стабильный hex на установку (несекретный файл рядом с настройками).
- Недостающие метаданные (артисты/альбом/обложка) добираются read-only запросом
  к каталогу по playible id, с LRU-кешем 200 записей; ошибки не блокируют
  live-state.

## ADR-005: Discord IPC через discord-rich-presence

- **Статус:** принято
- **Дата:** 2026-10-16

Crate `discord-rich-presence` 1.1 (MIT, активный). Синхронный API изолирован в
`discord::client`; клиент живет в отдельном std-потоке координатора. Бот-токен
не используется, только публичный Application ID через переменную окружения
сборки `DISCORD_APPLICATION_ID` (release-сборка без него невозможна —
build.rs падает с понятной ошибкой). Динамические обложки — прямые HTTPS URL
в `large_image` (официально поддерживается прокси Discord); fallback —
статический asset key из Developer Portal.

## ADR-006: Модель координатора Presence

- **Статус:** принято
- **Дата:** 2026-10-16

Единый std-поток-координатор (без блокировок общего состояния): события Ynison
по unbounded-каналу, решение publish/clear принадлежит только ему. Правила:
debounce 300 мс, rate limit ≤ 1 публикации / 4.5 с (перекрывает лимит Discord
5/20 c), grace-период 10 c между краткими разрывами, очистка активити на
Stopped/выключенной настройке паузы/глобальном выключении. Долгоживущие задачи
гасятся через `CancellationToken` от Supervisor.

## ADR-007: Раскладка источников `sources/` и два оркестратора

- **Статус:** принято
- **Дата:** 2026-10-18

Фактическая структура после рефактора (ТЗ §4.1 уточнено под PLAN п.2):

```text
src-tauri/src/
  sources/
    mod.rs      # контракт: SourceKind, SourceEvent
    model.rs    # нормализованная модель PlaybackState (ТЗ §5.3)
    ynison.rs   # тонкий адаптер над yandex::ynison_client
    smtc/
      mod.rs    # реэкспорты fallback-источника
      source.rs # polling-источник, выбор сессии, листинг для UI
      helpers.rs# await_smtc + smtc_reports_playing (коррекция ложной паузы)
  yandex/
    ynison_client.rs # реализация протокола (handshake, кадры, metadata)
    protocol.rs      # разбор кадров -> sources::model
    models.rs        # re-export sources::model (совместимость с ТЗ §4.1)
  presence/          # только решение publish/clear, без SMTC
```

Правила:

- `sources/` перечисляет ВСЕ источники. Реализация Ynison живёт в
  `yandex/` (специфика протокола), но регистрируется через
  `sources::ynison::spawn_ynison_source` — `runtime::Supervisor` не ходит в
  `yandex::ynison_client` напрямую.
- `presence/smtc.rs` удалён: SMTC-хелперы принадлежат `sources::smtc::helpers`.
- Два оркестратора вместо одного «координатора» из ТЗ §7:
  `runtime::Supervisor/Arbiter` решает *какой источник запущен* (режим
  `sourceMode`, fallback 10 c, sticky-выбор), а
  `presence::coordinator` решает *когда publish/clear в Discord* (debounce,
  rate limit, grace). Связка запускается в `lib.rs`.

## ADR-008: Настройки источника (расширение ТЗ §8)

- **Статус:** принято
- **Дата:** 2026-10-18

К схеме ТЗ §8 добавлены `sourceMode: auto | ynison | media_control`
(по умолчанию `auto`) и `smtcSourceApp: string` (пусто — автовыбор с
sticky + деприоритезацией браузеров, задано — строгая фиксация).
Команда `list_media_sessions` отдаёт видимые SMTC-сессии для выбора процесса
(PLAN п.2.1). Токен по-прежнему только в Credential Manager.


