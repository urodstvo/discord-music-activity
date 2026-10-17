# Yandex Music Presence

Лёгкое desktop-приложение для Windows: читает состояние воспроизведения Яндекс
Музыки (по неофициальному протоколу Ynison) и публикует его в Discord как
Rich Presence (Activity `LISTENING`). Работает из системного трея, потребляет
минимум ресурсов в простое.

Приложение **не является плеером**: оно не ищет треки по строкам, не скачивает
аудио и не управляет воспроизведением.

## Стек

- Backend: **Rust + Tauri 2** (WebSocket Ynison, Discord IPC, credential store)
- Frontend: **Vue 3 + TypeScript + Vite** (интерфейс, настройки, диагностика)
- Node.js используется только для сборки фронтенда — в runtime его нет

## Архитектура

```
┌─────────────┐   playback events   ┌──────────────────┐  presence  ┌─────────┐
│ Ynison WS-  │ ───────────────────▶│ Presence         │ ─────────▶ │ Discord │
│ client      │                     │ coordinator      │            │ IPC     │
│ (tokio)     │                     │ (std thread:     │            └─────────┘
└─────────────┘                     │ debounce, rate   │
       ▶ REST /tracks               │ limit, grace)    │
         (доделка метаданных)       └──────────────────┘
```

Ключевые модули backend (`src-tauri/src/`):

| Модуль | Назначение |
| --- | --- |
| `yandex/ynison_client.rs` | Жизненный цикл WS: redirect → state-поток, keep-alive, backoff |
| `yandex/protocol.rs`, `yandex/normalize.rs` | Толерантный разбор кадров, математика позиции |
| `yandex/metadata.rs` | Доделка метаданных по track id (read-only каталог, LRU-кеш) |
| `yandex/auth.rs` | Проверка/хранение/удаление OAuth-токена (Credential Manager) |
| `presence/coordinator.rs` | Решение «публиковать/очищать»: debounce, rate limit 5/20 c, grace 10 c |
| `presence/mapper.rs` | Чистое преобразование `PlaybackState → DiscordActivity` |
| `discord/client.rs` | IPC-адаптер discord-rich-presence, rate limiting |
| `config/`, `logging/` | Настройки (без секретов), ротация логов с защитой от утечек |
| `runtime.rs` | Supervisor долгоживущих задач (cancellation token) |

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
cargo test          # unit-тесты backend (протокол, mapper, backoff, настройки)
pnpm build          # typecheck + сборка фронтенда
```

## Первичный запуск

1. Запустите приложение — окно «Главная».
2. Вставьте OAuth-токен Яндекс Музыки (поле со скрытым вводом) и сохраните.
3. Запустите музыку — presence появится в профиле Discord.
4. Закрытие окна по умолчанию сворачивает приложение в трей; полный выход —
   в диагностике раздела «Завершить работу» или через меню трей.
