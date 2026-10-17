"""Перечислить Credential Manager записи по фильтру (без секретов)."""
import ctypes

# PCREDENTIALW** стиль вызова
class CREDENTIALW(ctypes.Structure):
    _fields_ = [
        ('Flags', ctypes.c_uint),
        ('Type', ctypes.c_uint),
        ('TargetName', ctypes.c_wchar_p),
        ('Comment', ctypes.c_wchar_p),
        ('LastWritten', ctypes.c_uint64),
        ('CredentialBlobSize', ctypes.c_uint),
        ('CredentialBlob', ctypes.c_void_p),
        ('Persist', ctypes.c_uint),
        ('AttributeCount', ctypes.c_uint),
        ('Attributes', ctypes.c_void_p),
        ('TargetAlias', ctypes.c_wchar_p),
        ('UserName', ctypes.c_wchar_p),
    ]

count = ctypes.c_uint()
arr = ctypes.POINTER(ctypes.POINTER(CREDENTIALW))()
ok = ctypes.windll.advapi32.CredEnumerateW(None, 0, ctypes.byref(count), ctypes.byref(arr))
print('enum ok =', ok, 'count =', count.value)
if ok:
    for i in range(count.value):
        cred = arr[i].contents
        blob = ''
        if cred.CredentialBlobSize:
            print(f'{cred.TargetName!r}: user={cred.UserName!r} blob={cred.CredentialBlobSize}B')
    ctypes.windll.advapi32.CredFree(arr)
