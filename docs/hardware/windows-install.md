# Windows: WinUSB для экранов S4 MK3

Кнопки, ручки и подсветка панели на Windows работают сразу: это HID-узлы, их отдаёт системный `hidusb`. Экраны — другое
дело: они живут на vendor-интерфейсе `interface 4` (bulk out `0x03`), и Windows отдаёт этот интерфейс драйверу Native
Instruments. Пока он не на WinUSB, `nusb` не может его занять, и служба говорит ровно это:

```
display interface 4 cannot be claimed: incompatible driver is installed for this interface
```

Рабочий путь владельца — **Zadig**: одна машина, одна панель, руки. Пакет для дистрибутива (`wdi-simple` + `pnputil`)
описан ниже, но нужен он только чтобы чужая машина обошлась без Zadig.

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

## Пакет для дистрибутива: wdi-simple + pnputil

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
