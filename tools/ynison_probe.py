"""Эталонный пробник Ynison.

Подключается к Ynison ОФИЦИАЛЬНОЙ python-библиотекой (`yandex-music`,
reference- SDK, на котором основан исследовательский референс) и печатает
каждый пришедший кадр состояния. Служит ground truth для сравнения с нашим
Rust-клиентом:

- если SDK получает кадры и держит соединение, а наше приложение получает
  503 — проблема в наших байтах (сравним и починим наш клиент);
- если SDK тоже получает "Ynison service is temporary unavailable" —
  проблема на стороне Яндекса (регион/сервис), наш код тут ни при чём.

Установка и запуск из корня проекта:
    py -m pip install -U yandex-music
    py tools/ynison_probe.py <ваш_токен_Яндекс_Музыки>

Токен не сохраняется, передаётся только в память процесса.
"""

from __future__ import annotations

import asyncio
import logging
import sys

logging.basicConfig(
    level=logging.DEBUG,
    format='%(asctime)s.%(msecs)03d %(levelname)s %(name)s: %(message)s',
    datefmt='%H:%M:%S',
)
logging.getLogger('websockets').setLevel(logging.INFO)
logging.getLogger('urllib3').setLevel(logging.INFO)

log = logging.getLogger('ynison_probe')


def state_summary(client: object) -> str:
    """Короткая безсекретная сводка последнего кадра состояния."""
    try:
        state = client.latest_state  # type: ignore[attr-defined]
        queue = state.player_state.player_queue
        idx = getattr(queue, 'current_playable_index', -1)
        playable = getattr(queue, 'playable_list', []) or []
        track = playable[idx] if 0 <= idx < len(playable) else None
        title = getattr(track, 'title', None)
        paused = state.player_state.status.paused
        return f'track={title!r} paused={paused} index={idx} queue_len={len(playable)}'
    except Exception as e:  # noqa: BLE001
        return f'(нет разбора состояния: {e!r})'


DEFAULT_CRED_TARGET = 'yandex-music-oauth-token.yandex-music-presence'


def read_credential(target: str) -> str:
    """Прочитать пароль из Windows Credential Manager по target-имени.

    Секрет не печатается и не сохраняется — только в памяти процесса.
    """
    import ctypes
    from ctypes import wintypes

    class CREDENTIALW(ctypes.Structure):
        _fields_ = [
            ('Flags', ctypes.c_uint),
            ('Type', ctypes.c_uint),
            ('TargetName', wintypes.LPCWSTR),
            ('Comment', wintypes.LPCWSTR),
            ('LastWritten', ctypes.c_uint64),
            ('CredentialBlobSize', ctypes.c_uint),
            ('CredentialBlob', ctypes.c_void_p),
            ('Persist', ctypes.c_uint),
            ('AttributeCount', ctypes.c_uint),
            ('Attributes', ctypes.c_void_p),
            ('TargetAlias', wintypes.LPCWSTR),
            ('UserName', wintypes.LPCWSTR),
        ]

    pc = ctypes.POINTER(CREDENTIALW)()
    # CRED_TYPE_GENERIC = 1 (записи keyring-crate — LegacyGeneric).
    if not ctypes.windll.advapi32.CredReadW(target, 1, 0, ctypes.byref(pc)):
        raise OSError(f'CredReadW({target!r}) failed: {ctypes.GetLastError()}')
    try:
        cred = pc.contents
        blob = ctypes.create_string_buffer(cred.CredentialBlobSize)
        ctypes.memmove(blob, cred.CredentialBlob, cred.CredentialBlobSize)
        return blob.raw.decode('utf-16-le')
    finally:
        ctypes.windll.advapi32.CredFree(pc)


async def main() -> int:
    token = ''
    if len(sys.argv) >= 2 and sys.argv[1].strip():
        token = sys.argv[1].strip()
    else:
        # Токен остаётся в Credential Manager: читаем НЕ выводя его в консоль.
        import ctypes
        token = read_credential(DEFAULT_CRED_TARGET).strip()
        if token:
            log.info('Токен прочитан из Credential Manager (не выводится)')
    if not token:
        print(__doc__)
        return 2

    try:
        from yandex_music import YnisonClientAsync
    except ImportError:
        try:
            from yandex_music.ynison import YnisonClientAsync
        except ImportError:
            log.error('Библиотека yandex-music не найдена: py -m pip install -U yandex-music')
            return 2

    client = YnisonClientAsync(token, device_title='Ynison Probe')
    frames = 0

    def on_state(state):  # noqa: ANN001
        nonlocal frames
        frames += 1
        log.info('STATE #%d: %s', frames, state_summary(client))

    client.on_state(on_state)

    def on_error(err):  # noqa: ANN001
        log.warning('SDK non-terminal error: %r', err)

    client.on_error(on_error)

    log.info('Подключаюсь (device_id=%s)...', client.device_id)
    try:
        await client.connect()
    except Exception as e:  # noqa: BLE001
        log.error('connect завершён: %r', e)
        return 1 if frames == 0 else 0
    log.info('Отключено; всего кадров: %d', frames)
    return 0


if __name__ == '__main__':
    sys.exit(asyncio.run(main()))
