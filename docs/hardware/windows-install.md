# Windows: WinUSB для экранов S4 MK3

Кнопки, ручки и подсветка панели на Windows работают сразу: это HID-узлы, их отдаёт системный `hidusb`. Экраны — другое
дело: они живут на vendor-интерфейсе `interface 4` (bulk out `0x03`), и Windows отдаёт этот интерфейс драйверу Native
Instruments. Пока он не на WinUSB, `nusb` не может его занять, и служба говорит ровно это:

```
display interface 4 cannot be claimed: incompatible driver is installed for this interface
```

Для дистрибутива это одна команда — `dj-controller --screens-driver` (раздел «Дистрибутив» ниже). Рабочий путь
владельца на своей машине — **Zadig**: одна панель, руки, без пакета.

## Дистрибутив ставится на любой машине

В пакете (`dist/windows`, zip и msi) рядом с программами лежит каталог `driver/`: `.inf`, `.cat`, `driver.cer` и
коинсталлеры WinUSB для amd64 и x86. Его берут из дерева (`assets/windows/driver`), `windows.sh --dist` копирует как
есть. Этого достаточно, чтобы привязать интерфейс на чистой машине:

```
dj-controller --screens-driver
```

**Из msi шаг делается сам.** Установщик уже администратор, поэтому в его последовательность добавлено отложенное
действие `BindScreensDriver` (тип `3154`: exe из установленного файла, отложенный скрипт, без олицетворения — то есть
от SYSTEM, и `Return="ignore"`): после того как файлы на месте, `dj-controller.exe --screens-driver --elevated`
связывает `MI_04`. Отказ драйвера не отменяет установку приложения — программа без экранов всё ещё программа.
Ответ шага, если что-то пошло не так, лежит в журнале SYSTEM:
`C:\Windows\System32\config\systemprofile\AppData\Roaming\hauler-dj\logs\screens-driver.log`.

- **Права команда просит сама** (`runas`, обычное окно UAC): драйвер ставится только от администратора. Elevated-копия
  — та же программа с внутренним `--elevated`; она кладёт `driver.cer` в `Root` и `TrustedPublisher`, запускает
  `pnputil /add-driver … /install`, а её ответ возвращается в консоль, откуда команду позвали (у elevated-копии свой
  консольный выход, и он закрывается вместе с ней).
- **В конце команда проверяет результат делом:** пробует открыть интерфейс 4, говорит, открылся ли он, и выходит с
  кодом 2, если нет. Верить надо этому вердикту, а не ответу установщика: драйвер считается поставленным только когда
  интерфейс открывается.
- **Повторный запуск безопасен:** если интерфейс уже на WinUSB, команда так и говорит и прав не просит.
- **`--screens-driver --check`** ничего не меняет и прав не просит: что нашлось рядом с программами, подключена ли
  панель, открывается ли интерфейс.

Зачем `driver.cer`: `.cat` подписан **самоподписанным** сертификатом libwdi, и на машине, где пакет делали, этот
сертификат уже лежит в `Root` и `TrustedPublisher`. На любой другой машине его нет, и без `driver.cer` `pnputil`
откажет по подписи — поэтому сертификат едет в пакете и ставится до драйвера.

msi кладёт те же файлы в `Program Files\Hauler DJ` (каталог `driver/` внутри), привязка интерфейса — та же команда.

## Zadig: связать интерфейс 4

1. Zadig → **Options → List All Devices**. Без этой галочки видны только устройства без драйвера, а у панели драйвер
   есть — то есть ровно тот случай, ради которого всё делается.
2. В списке выбрать строку с **`Interface 4`**. Панель показывается несколькими строками (по одной на интерфейс), и
   `Interface 3` рядом — vendor-коллекция HID: WinUSB там заберёт у `hidusb` кнопки, ручки и подсветку, которые уже
   работают, а экраны останутся закрытыми.
3. Драйвер — **WinUSB** (в Zadig это `WinUSB (v6.1.7600.16385)`), дальше **Replace Driver** (если драйвера нет —
   кнопка называется Install Driver).
4. Дождаться `The driver was installed successfully`. Zadig сам подписывает пакет своим сертификатом и сам кладёт его в
   `Root` и `TrustedPublisher` этой машины, поэтому ни `certutil`, ни `pnputil` не нужны.

Проверка — без гаданий:

```
dj-controller --screens-driver --check     # "interface 4 opens: WinUSB is bound and the screens are reachable"
dj-controller --screen-send                # кадр на обе панели
```

Если номер интерфейса в списке Zadig не виден (имя строки — то, что даёт Windows), его показывает PowerShell:

```powershell
Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like "*VID_17CC&PID_1720*" } |
  Select-Object InstanceId, FriendlyName, Class, Status
```

`Interface 4` — это `USB\VID_17CC&PID_1720&MI_04`. Служба, которая сейчас на нём, видна так:

```powershell
Get-PnpDeviceProperty -InstanceId "USB\VID_17CC&PID_1720&MI_04" -KeyName DEVPKEY_Device_Service
```

До Zadig там будет служба NI, после — `WinUSB`.

## Если WinUSB уже посажен на интерфейс 3

Тогда панель «подключена, но нечитаема»: коллекция HID ушла от `hidusb`. Вернуть — снять пакет, а не переставлять:

```
pnputil /enum-drivers                 # найти oemNN.inf, где Provider = libwdi, а имя про Traktor Kontrol S4 MK3
pnputil /delete-driver oemNN.inf /uninstall /force
```

После этого выдернуть и вставить панель: интерфейс 3 снова займёт `hidusb`, кнопки вернутся. Экраны при этом останутся
закрытыми — их интерфейс 4, и его надо связать отдельно (Zadig, шаги выше).

## Как этот пакет собрать заново

Пакет в `assets/windows/driver` сделан один раз и лежит в дереве; пересобирать его нужно только если меняется
устройство или драйвер.

`wdi-simple.exe` из релизов libwdi пишет пакет WinUSB сам и не требует подключённой панели. Запускать **от
администратора**: каталог `.cat` создаётся и подписывается только с правами, иначе libwdi честно пишет
`No .cat file generated (missing elevated privileges)`.

```
wdi-simple.exe -n "Traktor S4 MK3 Display" -v 0x17cc -p 0x1720 -i 4 -f s4_mk3_display.inf -t 0 -d driver --extract
```

- `-i 4` — интерфейс экранов. **Не `3`**: `interface 3` — это vendor-коллекция HID, и WinUSB на ней отберёт у `hidusb`
  кнопки и ручки, которые уже работают.
- `-f s4_mk3_display.inf` — имя inf. Расширение обязательно: libwdi отвечает на голое имя
  `Inf name provided must have a '.inf' extension`.
- `-t 0` — WinUSB, единственный тип, который понимает `nusb`.
- `-d driver` — каталог, куда складывать; `--extract` — только сложить, не ставить.

В `driver/` окажутся `s4_mk3_display.inf`, `s4_mk3_display.cat` и каталоги `amd64`/`x86` с двоичными файлами WinUSB.

**Про сертификат.** Каталог подписывается самоподписанным сертификатом libwdi, и на машине, где пакет делали, этот
сертификат уже лежит в хранилищах `Root` и `TrustedPublisher` — поэтому там пакет ставится молча. На чужой машине его
нет, и Windows откажется ставить `.inf` по подписи. Закрывается это файлом `driver.cer`: его вынимают из `.cat`
(свойства файла → Цифровые подписи → Просмотр сертификата → Копировать в файл) и кладут рядом с `.inf`.

Каталог `driver/` вместе с `driver.cer` кладётся в `assets/windows/driver/` дерева: `windows.sh --dist` копирует его в
пакет как есть, и `--screens-driver` находит `.inf` сам (папка `driver/` рядом с программами).

Установка на чужой машине — та же команда `dj-controller --screens-driver`: она кладёт `driver.cer` в `Root` и
`TrustedPublisher` (если он есть рядом с `.inf`) и ставит пакет через `pnputil /add-driver … /install`, чтобы уже
подключённая панель переехала на WinUSB сразу, а не после перезагрузки. Без `.inf` в `driver/` команда так и говорит —
и называет, чего не хватает.
