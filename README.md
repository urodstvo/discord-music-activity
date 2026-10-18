# Yandex Music Presence

Лёгкое desktop-приложение для Windows: читает состояние воспроизведения и
публикует его в Discord как Rich Presence (Activity `LISTENING`). Работает
из системного трея, потребляет минимум ресурсов в простое.

Два источника данных (единый интерфейс `sources/`, нормализованная модель
`PlaybackState`):

- **Ynison** — real-time протокол Яндекс Музыки (нужен OAuth-токен). Точная
  позиция с сервера, метаданные, обложка, кнопка «Открыть в Яндекс Музыке».
- **Windows Media Control (SMTC)** — локальный fallback, работает **без
  токена и без сети**: читает медиа-сессии Windows (Яндекс Музыка, Spotify,
  браузеры и т.д.). Без кнопки и обложки (сетевого URL у локальных сессий
  нет), позиция — через timeline ОС.

Приложение **не является плеером**: оно не ищет треки по строкам, не скачивает
аудио и не управляет воспроизведением.

## Стек

- Backend: **Rust + Tauri 2** (Ynison WebSocket, SMTC, Discord IPC, credential store)
- Frontend: **Vue 3 + TypeScript + Vite** (интерфейс, настройки, диагностика)
- Node.js используется только для сборки фронтенда — в runtime его нет

## Архитектура

```
+----------------+  SourceEvent   +---------------+  CoordinatorEvent  +------------------+  presence  +---------+
| Ynison WS      | --------------->| Supervisor    | ------------------>| Presence         | ---------> | Discord |
| client (tokio) |                | (arbiter:     |                   | coordinator      |            | IPC     |
+----------------+                | sourceMode,   |                   | (std thread:     |            +---------+
+----------------+  SourceEvent   | pin, sticky)  |                   | debounce 300ms,  |
| SMTC polling   | --------------->|               |                   | rate limit 4.5s, |
| (~1s, COM)     |                +---------------+                   | grace 10s)       |
+----------------+                                                     +------------------+
        -> REST /tracks (доделка метаданных Ynison по track id)
```

Источники шлют только нормализованные события и ничего не знают про
Discord. Координатор — единственное место, решающее «публиковать/очищать».
`start` Activity считается от момента замера позиции (`observed_at`),
а не отправки — debounce/rate-limit точность не съедают.

Особенности SMTC-источника (важно для понимания поведения):

- **Фиксация процесса** (`smtcSourceApp`, Настройки → «Фиксация процесса»):
  строгий выбор одной медиа-сессии по app id; пусто — авто (sticky +
  предпочтение не-браузеров и текущей сессии ОС).
- **Мёртвый timeline**: некоторые плееры не двигают `Position`
  (напр. десктопная Яндекс Музыка отдаёт константу). Стоящая позиция не
  публикуется — часы идут локальным секундомером от последнего события,
  таймер Discord не сбрасывается.
- **Seek с подтверждением**: скачок позиции публикуется, только если
  следующий тик его подтверждает; откат к уже опубликованному значению
  (лаг буферизации при промотке) игнорируется.

Ключевые модули backend (`src-tauri/src/`):

| Модуль | Назначение |
| --- | --- |
| `sources/mod.rs`, `sources/model.rs` | Единый контракт источников, нормализованная модель |
| `sources/ynison.rs` | Адаптер Ynison как источника (регистрация в `sources/`) |
| `sources/smtc/source.rs` | SMTC polling, выбор сессии, sticky/pin, frozen/seek-логика |
| `sources/smtc/helpers.rs` | `await_smtc` + SMTC-сверка ложной паузы Ynison |
| `yandex/ynison_client.rs` | Жизненный цикл WS: redirect → state-поток, keep-alive, backoff |
| `yandex/models.rs` | Re-export `sources::model` (совместимость с ТЗ §4.1) |
| `yandex/protocol.rs` | Толерантный разбор кадров, математика позиции |
| `yandex/metadata.rs` | Доделка метаданных по track id (read-only каталог, LRU-кеш) |
| `yandex/auth.rs` | Проверка/хранение/удаление OAuth-токена (Credential Manager) |
| `presence/coordinator.rs` | Решение «публиковать/очищать»: debounce, rate limit, grace |
| `presence/mapper.rs` | Чистое преобразование `PlaybackState → DiscordActivity` |
| `discord/client.rs` | IPC-адаптер discord-rich-presence, rate limiting |
| `runtime.rs` | Supervisor: арбитр источников, Discord reset, shutdown |
| `commands/` | Узкий Tauri API: state/settings/auth/playback/`list_media_sessions` |
| `config/`, `logging/` | Настройки без секретов (schema v3), ротация логов |

Исследования протоколов — в `docs/research/` (Ynison handshake, Discord IPC),
архитектурные решения — `docs/architecture-decisions.md`.

## Безопасность данных

- OAuth-токен хранится **только** в Windows Credential Manager (`keyring`).
- Конфиг (`%APPDATA%\yandex-music-presence\settings.json`) не содержит секретов.
- Логи: ротация по дням, зарегистрированные секреты маскируются, токен и
  Authorization-заголовки не логируются.
- CSP запрещает внешние скрипты; обложки — только с hosts
  `avatars.yandex.net` / `avatars.mds.yandex.net` по HTTPS.

## Сборка и запуск

Требования: Rust (stable, MSVC toolchain), Node.js + pnpm, Windows 10/11 x64.

```bash
pnpm install

# 1) Application ID Discord (не секрет; создаётся в Developer Portal).
#    Для debug-сборки можно пропустить — тогда публикации вернут ошибку конфигурации.
#    Для release-сборки переменная ОБЯЗАТЕЛЬНА (сборка упадёт без неё).
#
#    ID читается из переменной окружения ИЛИ из файла .env (корень проекта).
#    Проще всего один раз скопировать .env.example в .env и вписать ID:
#      Copy-Item .env.example .env      # затем вписать DISCORD_APPLICATION_ID=...
#
#    Вариант с переменной окружения (сильнее, чем .env):
$env:DISCORD_APPLICATION_ID="123456789012345678"

# Режим разработки (окно + hot reload):
pnpm tauri dev

# Сборка релиза (NSIS/MSI):
pnpm tauri build
```

Распространять `DISCORD_APPLICATION_ID` не нужно — пользовательский токен
Discord не используется, только публичный Application ID приложения.

## Тесты

```bash
cargo test          # unit-тесты backend (протокол, mapper, SMTC, координатор, настройки)
pnpm build          # typecheck + сборка фронтенда
```

## Первичный запуск

1. Запустите приложение — окно «Главная».
2. Вариант А (точно и с обложкой): вставьте OAuth-токен Яндекс Музыки
   (поле со скрытым вводом) и сохраните — источником будет Ynison.
   Вариант Б (без токена): режим «Windows Media Control» в Настройках —
   presence берётся из локальных медиа-сессий.
3. Запустите музыку — presence появится в профиле Discord.
4. Если статус скачет между приложениями: Настройки → «Фиксация процесса» →
   «Обновить список» → выберите нужный процесс. Применяется сразу.
5. Закрытие окна по умолчанию сворачивает приложение в трей; полный выход —
   в диагностике раздела «Завершить работу» или через меню трей.