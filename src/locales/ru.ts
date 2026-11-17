// Русский каталог. Набор ключей должен совпадать с `en.ts` (проверяют компилятор и тест).
// Тон: спокойный, короткий, на «ты»; кнопки и заголовки — в инфинитиве или существительным.
// Названия продуктов (версии Minecraft, Fabric, Forge, NeoForge, Quilt, UUID, пути) и бренд
// Clade / Clade Launcher не переводятся.

import type { Message } from "@/lib/i18n/core";
import type { MessageKey } from "./index";

export const ru: Record<MessageKey, Message> = {
  // ---- common
  "common.cancel": "Отмена",
  "common.close": "Закрыть",
  "common.done": "Готово",
  "common.tryAgain": "Повторить",
  "common.working": "Выполняется…",
  "common.continue": "Продолжить",
  "common.unlock": "Разблокировать",
  "common.passphrase": "Парольная фраза",
  "common.signInAgain": "Войти снова",
  "common.openLogs": "Открыть логи",

  // ---- window chrome
  "window.minimize": "Свернуть",
  "window.close": "Закрыть",
  "window.wordmark": "Показать девиз Clade",

  // ---- home
  "hero.subtitle": "Выбери профиль и войди в свой мир.",
  "hero.shuffleHint": "Показывает другое сообщение",
  "hero.played": "В игре {time}",
  "phrase.ready-when-you-are": "Начнём, когда захочешь.",
  "phrase.your-world-is-waiting": "Твой мир ждёт.",
  "phrase.welcome-back": "С возвращением.",
  "phrase.pick-up-where-you-left-off": "Продолжим с того же места.",
  "phrase.where-to-today": "Куда сегодня?",
  "phrase.one-more-world": "Ещё один мир?",
  "phrase.take-your-time": "Не торопись.",
  "phrase.choose-your-world": "Выбери свой мир.",

  // ---- durations and sizes
  "playtime.zero": "0 мин",
  "playtime.lessThanMinute": "<1 мин",
  "playtime.minutes": "{minutes} мин",
  "playtime.hoursMinutes": "{hours} ч {minutes} мин",
  "size.kb": "{value} КБ",
  "size.mb": "{value} МБ",
  "size.gb": "{value} ГБ",

  // ---- dock: play button
  "play.play": "Играть",
  "play.setup": "Подготовить",
  "play.launching": "Запуск…",
  "play.playing": "В игре",
  "play.repairing": "Восстановление…",
  "play.signIn": "Войти для продолжения",

  // ---- dock: settings button
  "dock.settings": "Настройки",

  // ---- dock: account menu
  "account.signIn": "Войти",
  "account.trigger": "Аккаунт: {name}. Сменить аккаунт",
  "account.triggerNone": "Добавить аккаунт",
  "account.needsSignIn": "Войти снова",
  "account.manage": "Управление: {name}",
  "account.add": "+ Добавить аккаунт",

  // ---- dock: profile selector
  "profile.createFirst": "Создать профиль",
  "profile.trigger": "Профиль: {label}. Сменить профиль",
  "profile.create": "+ Создать профиль",
  "profile.verify": "Проверить файлы",
  "profile.remove": "Убрать из Clade…",
  "removeProfile.title": "Убрать «{profile}» из Clade?",
  "removeProfile.body":
    "Профиль исчезнет из Clade, но его миры, моды, настройки, скриншоты и другие файлы останутся сохранены в папке Clade. Ничего не будет удалено безвозвратно.",
  "removeProfile.confirm": "Убрать из Clade",
  "removeProfile.working": "Убираем…",
  "profile.vanilla": "Ванильный",
  "profile.modded": "С модами",

  // ---- create profile
  "create.title": "Новый профиль",
  "create.description": "У каждого профиля свои файлы, отдельно от остальных.",
  "create.name": "Название",
  "create.namePlaceholder": "Выживание",
  "create.mcVersion": "Версия Minecraft",
  "create.filterLabel": "Какие версии показывать",
  "create.filter.releases": "Релизы",
  "create.filter.snapshots": "Снапшоты",
  "create.filter.historical": "Старые",
  "create.choose": "Выбери версию",
  "create.loading": "Загрузка версий…",
  "create.unavailable": "Версии недоступны",
  "create.noVersions": "В этом списке нет версий.",
  "create.type": "Тип",
  "create.typeLabel": "Тип профиля",
  "create.vanillaNote": "Обычный Minecraft, ничего не добавлено.",
  "create.moddedNote": "Minecraft с загрузчиком модов. Моды добавляются отдельно.",
  "create.loader": "Загрузчик",
  "create.loaderLabel": "Загрузчик модов",
  "create.loaderCheckFailed":
    "Не удалось проверить, какие загрузчики подходят. Проверь подключение.",
  "create.loaderChecking": "Проверка подходящих загрузчиков…",
  "create.loaderNone": "Для {minecraft} нет версии {loader}",
  "create.loaderNoneAny": "Для этой версии нет {loader}",
  "create.loaderUnchecked": "Не удалось проверить {loader}",
  "create.loaderVersion": "Версия загрузчика",
  "create.versionRecommended": "{version} (рекомендуется)",
  "create.channel.recommended": "рекомендуется",
  "create.channel.latest": "новейшая",
  "create.channel.prerelease": "бета",
  "create.channel.stable": "",
  "create.checking": "Проверка…",
  "create.noCompatible": "Нет подходящей версии",
  "create.staleLoaders": "Нет сети: используется сохранённый список версий.",
  "create.creating": "Создание…",
  "create.submit": "Создать профиль",
  "validate.nameEmpty": "Дай профилю название.",
  "validate.nameLong": {
    by: "max",
    one: "Название не должно быть длиннее {max} символа.",
    other: "Название не должно быть длиннее {max} символов.",
  },
  "validate.nameControl": "В названии не должно быть управляющих символов.",
  "validate.nameDuplicate": "Профиль с таким названием уже есть.",
  "catalog.stale": "Нет сети: используется список версий от {date}.",
  "catalog.staleUnknown": "Нет сети: используется сохранённый список версий.",

  // ---- preparing a profile
  "prepare.stage.planning": "Подготовка Minecraft",
  "prepare.stage.java": "Подготовка Java",
  "prepare.stage.minecraft": "Загрузка Minecraft",
  "prepare.stage.minecraft.verify": "Проверка Minecraft",
  "prepare.stage.libraries": "Загрузка библиотек",
  "prepare.stage.java.verify": "Проверка Java",
  "prepare.stage.libraries.verify": "Проверка библиотек",
  "prepare.stage.assets": "Загрузка ресурсов",
  "prepare.stage.assets.verify": "Проверка ресурсов",
  "prepare.stage.natives": "Распаковка нативных библиотек",
  "prepare.stage.loader": "Установка {loader}",
  "prepare.stage.verifying": "Проверка файлов",
  "prepare.stage.done": "Готово к игре.",
  "prepare.ready": "Готово к игре.",
  "prepare.readyDescription": "{name} установлен на этом компьютере.",
  "prepare.progress": "Ход подготовки",
  "prepare.files": {
    by: "total",
    one: "{done} из {total} файла",
    few: "{done} из {total} файлов",
    many: "{done} из {total} файлов",
    other: "{done} из {total} файла",
  },
  "prepare.bytes": "{done} из {total}",

  // ---- when the game could not start or ended badly
  "problem.endedTitle": "Minecraft остановился",
  "problem.title.no_account": "Добавить аккаунт",
  "problem.title.needs_sign_in": "Войти снова",
  "problem.title.needs_repair": "Нужно восстановление",
  "problem.title.approval_pending": "Вход пока недоступен",
  "problem.title.offline": "Нет подключения",
  "problem.title.other": "Не удаётся запустить",
  "problem.repair": "Восстановить",
  "problem.addAccount": "Добавить аккаунт",
  "problem.unlockTitle": "Разблокировать аккаунты",
  "problem.unlockDescription":
    "Введи парольную фразу, чтобы играть. Она остаётся на этом компьютере.",
  "exit.wrong_java":
    "Minecraft или мод требует другую версию Java, не ту, что подготовил Clade. Может помочь восстановление профиля.",
  "exit.incomplete_files": "Файлы Minecraft неполные. Восстанови профиль, чтобы скачать их заново.",
  "exit.no_memory":
    "Не хватает свободной памяти для запуска Minecraft. Закрой другие программы и попробуй снова.",
  "exit.out_of_memory": "Minecraft не хватило памяти.",
  "exit.native_library": "Не удалось загрузить нативную библиотеку (графика или звук).",
  "exit.java_crash": "Java аварийно завершилась — часто из-за драйвера видеокарты.",
  "exit.mod_loader": "Загрузчик модов сообщил об ошибке при запуске.",
  "exit.mixin": "Не удалось применить мод. Проверь моды в этом профиле.",
  "exit.incompatible_mods": "Моды в этом профиле несовместимы друг с другом.",
  "exit.missing_class": "Не удалось загрузить библиотеку или мод.",
  "exit.jvm_start": "Java не смогла запуститься с выбранными настройками.",
  "exit.early_exit": "Minecraft остановился сразу после запуска (код выхода {code}).",
  "exit.unexpected": "Minecraft неожиданно остановился (код выхода {code}).",
  "exit.stopped": "Minecraft был остановлен.",
  "exit.generic": "Minecraft неожиданно остановился.",

  // ---- sign-in and the account vault
  "signin.addTitle": "Добавить аккаунт",
  "signin.againTitle": "Войти снова",
  "signin.addDescription": "Войди через аккаунт Microsoft в браузере.",
  "signin.againDescription": "Войди как {name} через Microsoft.",
  "signin.stage.opening": "Открытие входа Microsoft…",
  "signin.stage.waiting": "Ожидание входа…",
  "signin.stage.finishing": "Завершение настройки…",
  "signin.createTitle": "Защита аккаунтов",
  "signin.createDescription":
    "Данные для входа шифруются в этой папке парольной фразой, которую ты выберешь. Восстановить её нельзя, поэтому храни её надёжно.",
  "signin.unlockTitle": "Разблокировать аккаунты",
  "recovery.link": "Не помнишь парольную фразу?",
  "recovery.title": "Начать заново?",
  "recovery.body":
    "Clade не может восстановить забытую парольную фразу. Если начать заново, старое зашифрованное хранилище входов будет убрано в архив, и эти сохранённые входы перестанут использоваться в этой папке Clade. Тебе придётся снова войти в аккаунты Microsoft. Профили, миры, время игры и настройки сохранятся.",
  "recovery.keep": "Продолжить попытки",
  "recovery.confirm": "Начать заново",
  "signin.unlockDescription":
    "Введи парольную фразу, чтобы войти. Она остаётся на этом компьютере.",
  "signin.passphraseNote": {
    one: "Не менее {count} символа. Подойдёт несколько слов.",
    other: "Не менее {count} символов. Подойдёт несколько слов.",
  },
  "signin.repeat": "Парольная фраза ещё раз",
  "signin.mismatch": "Парольные фразы не совпадают.",
  "signin.approvalNote":
    "Ничего не сохранено. Это исправится, когда Mojang одобрит регистрацию лаунчера.",

  // ---- account management
  "accountModal.description": "Аккаунт Minecraft Java",
  "accountModal.profile": "Профиль",
  "accountModal.skin": "Скин",
  "accountModal.cape": "Плащ",
  "accountModal.name": "Имя",
  "accountModal.uuid": "UUID",
  "accountModal.status": "Статус",
  "accountModal.statusNeedsSignIn": "Нужно войти снова",
  "accountModal.statusSignedIn": "Вход выполнен",
  "accountModal.skinModel": "Модель скина",
  "accountModal.skinClassic": "Классическая",
  "accountModal.skinSlim": "Тонкая",
  "accountModal.choosePng": "Выбрать PNG…",
  "accountModal.restoreDefault": "Вернуть стандартный",
  "accountModal.uploading": "Загрузка…",
  "accountModal.resetConfirm":
    "Вернуть стандартный скин? Текущий пользовательский скин будет удалён с этого аккаунта Minecraft.",
  "accountModal.keepSkin": "Оставить текущий скин",
  "accountModal.restoring": "Возврат…",
  "accountModal.skinModelNote": "Модель применяется к PNG, который ты выберешь следующим.",
  "accountModal.capeAvailable": "Выбери один из плащей этого аккаунта или отключи плащ.",
  "appearance.loading": "Загрузка…",
  "appearance.noCapes": "У этого аккаунта нет плащей.",
  "appearance.capeUnnamed": "Плащ {n}",
  "appearance.equip": "Надеть",
  "appearance.equipped": "Надет",
  "appearance.equipNamed": "Надеть «{name}»",
  "appearance.hideCape": "Скрыть плащ",
  "appearance.needsSignIn": "Войди снова, чтобы менять внешний вид в Minecraft.",
  "accountModal.confirmRemove": "Удалить {name}? Профили, миры и время игры останутся как есть.",
  "accountModal.keep": "Оставить аккаунт",
  "accountModal.remove": "Удалить аккаунт",
  "accountModal.removing": "Удаление…",
  "accountModal.signOutRemove": "Выйти и удалить…",

  // ---- settings
  "settings.title": "Настройки",
  "settings.sound": "Звуковые эффекты",
  "settings.volume": "Громкость",
  "settings.language": "Язык",
  "settings.interface": "Интерфейс",
  "settings.minecraft": "Игра",
  "settings.behavior": "Поведение",
  "settings.support": "Поддержка",
  "settings.motion": "Меньше анимации",
  "settings.motion.system": "Как в Windows",
  "settings.motion.on": "Вкл.",
  "settings.motion.off": "Выкл.",
  "settings.transparency": "Эффекты прозрачности",
  "settings.transparency.system": "Как в Windows",
  "settings.transparency.on": "Вкл.",
  "settings.transparency.off": "Выкл.",
  "settings.memory": "Память",
  "settings.memory.auto": "Автоматически (рекомендуется)",
  "settings.memory.custom": "Своя",
  "settings.memory.autoNote": "Подбирается автоматически: {value}.",
  "settings.memory.slider": "Память для Minecraft",
  "settings.memory.gb": "{value} ГБ",
  "settings.onLaunch": "Когда Minecraft запускается",
  "settings.onLaunch.keep": "Оставить Clade открытым",
  "settings.onLaunch.hide": "Скрыть Clade",
  "settings.onExit": "Когда Minecraft закрывается",
  "settings.onExit.show": "Показать Clade снова",
  "settings.onExit.hidden": "Оставить скрытым",
  "settings.onExit.hiddenNote":
    "Скрытый Clade тихо закроется вместе с игрой. Запусти его снова в любой момент.",
  "settings.openFolder": "Открыть папку Clade",
  "settings.exportDiagnostics": "Экспорт диагностики",
  "settings.exporting": "Экспорт…",
  "settings.diagnosticsNote": "В диагностике нет паролей, токенов и миров.",
  "settings.diagnosticsDone": "Сохранено: {name}.",
  "settings.reset": "Сбросить настройки",
  "settings.resetConfirm":
    "Сбросить настройки по умолчанию? Профили, миры, аккаунты и время в игре останутся как есть.",
  "settings.resetKeep": "Оставить",
  "settings.resetDo": "Сбросить",
  "settings.cat": "Кот",
  "onboarding.step": "Шаг {current} из {total}",
  "onboarding.skip": "Пропустить",
  "onboarding.back": "Назад",
  "onboarding.continue": "Продолжить",
  "onboarding.welcomeTitle": "Добро пожаловать в Clade",
  "onboarding.tagline": "Просто. Портативно. Твоё.",
  "onboarding.welcomeText": "Чистый портативный лаунчер для Minecraft: Java Edition.",
  "onboarding.portableTitle": "Портативность по замыслу",
  "onboarding.portableText":
    "Всё, чем управляет Clade, хранится в папке Clade. Её можно перенести на другой диск или на другой компьютер с Windows.",
  "onboarding.readyTitle": "Всё готово",
  "onboarding.readyText": "Создай профиль, добавь аккаунт Microsoft или просто осмотрись.",
  "onboarding.createProfile": "Создать профиль",
  "onboarding.addAccount": "Добавить аккаунт Microsoft",
  "onboarding.finish": "Готово",
  "error.settings.unknown": "Не получилось.",
  "error.settings.diagnostics_failed":
    "Не удалось создать файл диагностики. Проверь, что на диске есть место и он не защищён от записи.",
  "error.settings.invalid_memory":
    "Такой объём памяти небезопасен для этого компьютера. Выбери от {min} до {max}.",
  "error.settings.unsupported_value": "Такая настройка не поддерживается.",
  "error.settings.unsupported_language": "Этот язык не поддерживается.",
  "error.settings.open_failed": "Windows не смог открыть эту папку.",

  // ---- errors from the launcher's backend, worded by code (see lib/i18n/errors.ts)
  "error.generic": "Что-то пошло не так.",
  "error.common.storage":
    "Clade не удалось прочитать или записать нужные файлы. Проверь, что в его папку можно писать.",
  "error.common.not_found": "Этого профиля больше нет.",
  "error.common.network_unavailable": "Сеть недоступна.",
  "error.common.backend_unavailable": "Здесь лаунчер недоступен.",
  "error.common.unsupported_folder_path":
    "Clade не может работать из этой папки. Перемести папку Clade на обычный локальный диск и попробуй снова.",

  "error.launch.unknown": "Не удалось запустить Minecraft.",
  "error.launch.no_account": "Войди через аккаунт Microsoft, чтобы играть.",
  "error.launch.needs_sign_in": "Этому аккаунту нужно войти снова, чтобы играть.",
  "error.launch.needs_repair":
    "Часть файлов профиля отсутствует или повреждена. Перед запуском их нужно восстановить.",
  "error.launch.vault_locked": "Разблокируй аккаунты, чтобы играть.",
  "error.launch.offline":
    "Без сети Clade не может обновить вход в Minecraft. Подключись и повтори.",
  "error.launch.approval_pending":
    "Mojang ещё не одобрил этот лаунчер для входа в Minecraft, поэтому новый вход выполнить нельзя.",
  "error.launch.no_java_profile": "У этого аккаунта Microsoft нет профиля Minecraft: Java Edition.",
  "error.launch.auth": "Не удалось обновить вход в Minecraft.",
  "error.launch.not_ready": "Профиль нужно подготовить перед запуском.",
  "error.launch.unsupported": "Этот профиль не удаётся запустить.",
  "error.launch.already_running": "Этот профиль уже запускается или запущен.",
  "error.launch.already_preparing": "Этот профиль сейчас готовится. Дождись завершения.",
  "error.launch.spawn_failed": "Не удалось запустить Java.",

  "error.prepare.unknown": "Не удалось подготовить профиль.",
  "error.prepare.offline":
    "Нет доступа к сети, а часть нужных профилю файлов ещё не сохранена на этом компьютере.",
  "error.prepare.network": "Не удалось скачать файл. Проверь подключение и повтори.",
  "error.prepare.verification": "Скачанный файл не прошёл проверку и был удалён. Повтори попытку.",
  "error.prepare.metadata": "Информация Mojang об этой версии недоступна.",
  "error.prepare.unsupported": "Эту версию не удаётся подготовить.",
  "error.prepare.java": "Не удалось настроить Java.",
  "error.prepare.loader": "Не удалось установить загрузчик модов.",
  "error.prepare.storage":
    "Не удалось сохранить файлы. Проверь, есть ли место на диске и можно ли на него писать.",
  "error.prepare.disk_space":
    "На диске, где находится Clade, не хватает места: нужно около {required}, доступно {available}. Освободи место и повтори.",
  "error.prepare.account_required":
    "Войди в аккаунт Minecraft, чтобы скачивать или проверять файлы игры. Профили останутся как есть.",
  "error.prepare.busy": "Сейчас готовится другой профиль. Дождись завершения.",
  "error.prepare.already_running":
    "Этот профиль запускается или запущен. Закрой игру, прежде чем готовить или проверять его.",

  "error.signIn.unknown": "Не удалось войти.",
  "error.signIn.timed_out":
    "Вход занял слишком много времени и был остановлен. Повтори, когда захочешь.",
  "error.signIn.browser_unavailable": "Clade не удалось открыть браузер.",
  "error.signIn.in_progress": "Вход уже выполняется.",
  "error.signIn.offline": "Не удалось связаться с Microsoft. Проверь подключение и повтори.",
  "error.signIn.oauth": "Microsoft не принял вход.",
  "error.signIn.needs_sign_in": "Этому аккаунту нужно войти снова.",
  "error.signIn.no_xbox_account":
    "У этого аккаунта Microsoft нет аккаунта Xbox. Создай его на xbox.com и повтори.",
  "error.signIn.xbox_region": "Xbox Live недоступен в стране или регионе этого аккаунта.",
  "error.signIn.adult_verification":
    "Этому аккаунту нужно подтверждение возраста. Заверши его на xbox.com.",
  "error.signIn.child_account":
    "Это детский аккаунт. Взрослый должен добавить его в семейную группу Microsoft, прежде чем он сможет войти.",
  "error.signIn.xbox_banned": "Этот аккаунт Xbox заблокирован.",
  "error.signIn.xsts": "Не удалось войти в Xbox (код {xstsCode}).",
  "error.signIn.approval_required":
    "Вход через Microsoft прошёл, но Mojang ещё не одобрил этот лаунчер для входа в Minecraft.",
  "error.signIn.no_java_profile": "У этого аккаунта Microsoft нет Minecraft: Java Edition.",
  "error.signIn.no_java_profile_owned":
    "У этого аккаунта есть Minecraft, но ещё нет профиля Java. Открой официальный Minecraft Launcher один раз, чтобы создать его, и повтори.",
  "error.signIn.bad_response": "Вход в Minecraft вернул неожиданный ответ.",
  "error.signIn.vault_locked": "Сначала разблокируй аккаунты.",
  "error.signIn.storage": "Не удалось сохранить аккаунты.",
  "error.signIn.wrong_account":
    "Это другой аккаунт Minecraft, не тот, в который нужно войти снова.",
  "error.signIn.unknown_account": "Этого аккаунта больше нет.",
  "error.signIn.sign_in_unavailable": "Вход через Microsoft пока недоступен в этой сборке.",

  "error.vault.unknown": "Не получилось.",
  "error.vault.passphrase_mismatch": "Парольная фраза не подходит.",
  "error.vault.passphrase_too_short": "Парольная фраза слишком короткая.",
  "error.vault.passphrase_too_long": "Парольная фраза слишком длинная.",
  "error.vault.vault_exists": "Аккаунты уже защищены парольной фразой.",
  "error.vault.vault_unreadable": "Не удалось открыть зашифрованное хранилище аккаунтов.",
  "error.vault.vault_missing":
    "Зашифрованное хранилище аккаунтов не найдено в этой папке. Начни заново, чтобы войти снова.",
  "error.vault.sign_in_in_progress": "Идёт вход. Сначала заверши или отмени его.",
  "error.vault.appearance_busy":
    "Смена скина или плаща ещё выполняется. Попробуй через пару секунд.",
  "error.vault.game_starting": "Игра запускается. Попробуй, когда она запустится.",
  "error.vault.vault_unlocked": "Аккаунты уже разблокированы.",
  "error.vault.reset_failed":
    "Clade не смог убрать в архив старое хранилище входов, поэтому ничего не изменилось. Закрой всё, что использует папку Clade, и попробуй ещё раз.",
  "error.vault.accounts_not_updated":
    "Старое хранилище входов убрано в архив, но список аккаунтов обновить не удалось. Попробуй «Начать заново» ещё раз.",

  "error.account.unknown": "Не удалось удалить аккаунт.",
  "error.account.unknown_account": "Этого аккаунта больше нет.",

  "error.appearance.unknown": "Не удалось применить изменение.",
  "error.appearance.invalid_skin": "Выбери PNG-скин 64×64 или 64×32.",
  "error.appearance.needs_sign_in": "Войди снова, чтобы менять внешний вид в Minecraft.",
  "error.appearance.vault_locked": "Разблокируй аккаунты, чтобы менять внешний вид в Minecraft.",
  "error.appearance.offline": "Подключись к интернету, чтобы менять внешний вид в Minecraft.",
  "error.appearance.rate_limited": "Слишком много смен скина. Попробуй позже.",
  "error.appearance.unauthorized": "Minecraft не принял твой вход. Войди снова и повтори.",
  "error.appearance.forbidden": "Minecraft отклонил это изменение для аккаунта.",
  "error.appearance.cape_not_owned": "У этого аккаунта нет такого плаща.",
  "error.appearance.service_unavailable": "Сервисы Minecraft сейчас недоступны. Попробуй позже.",
  "error.appearance.no_profile": "У этого аккаунта нет профиля Minecraft: Java Edition.",
  "error.appearance.bad_response": "Minecraft прислал неожиданный ответ. Попробуй позже.",
  "error.appearance.storage": "Не удалось прочитать аккаунты.",
  "error.appearance.busy": "Другое изменение ещё выполняется.",
  "error.appearance.unknown_account": "Этого аккаунта больше нет.",

  "error.profile.unknown": "Не удалось создать профиль.",
  "error.profile.duplicate_name": "Профиль «{name}» уже существует.",
  "error.profile.unknown_minecraft":
    "Minecraft {minecraft} нет в официальном списке версий Mojang.",
  "error.profile.cannot_verify":
    "Сейчас не удаётся проверить версии Minecraft. Подключись к сети один раз и повтори.",
  "error.profile.cannot_verify_loader":
    "Сейчас не удаётся проверить, какие версии {loader} подходят для Minecraft {minecraft}. Подключись к сети один раз и повтори.",
  "error.profile.no_compatible_loader": "Для Minecraft {minecraft} нет версии {loader}.",
  "error.profile.loader_version_not_offered":
    "{loader} {version} не подходит для Minecraft {minecraft}.",
  "error.profile.invalid_name": "Такое название использовать нельзя.",
  "error.profile.storage": "Не удалось сохранить профиль.",
  "error.profile.remove_failed":
    "Не удалось переместить профиль. Закрой всё, что использует его папку (Проводник, редактор), и попробуй снова. Ничего не изменилось.",
  "error.profile.remove_refused": "Этот профиль нельзя убрать отсюда. Ничего не изменилось.",
  "error.profile.already_running": "Этот профиль запускается или уже запущен. Сначала закрой игру.",
  "error.profile.busy": "Этот профиль сейчас готовится. Дождись завершения.",

  "error.catalog.unknown": "Не удалось загрузить список версий.",
  "error.catalog.catalog_unavailable":
    "Не удалось загрузить список версий. Проверь подключение и повтори.",
};
