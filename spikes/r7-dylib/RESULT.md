# R7 — режимы отказа dylib-плагина

Спайк: `spikes/r7-dylib/{api,plugin,host}`, воспроизводит `./run.sh` (9 блоков, все assert'ы прошли, exit 0).
abi_stable 0.11.3, rustc 1.99.0, release; ABI — две `extern "C" fn`.

## Вердикт

- **panic** — ловится, но не библиотекой: host печатает `result: Err("caught panic at ABI boundary: spike panic")`, `still alive, exiting 0`, код **0**. Ловит ручной `catch_unwind` в `call`; без него (`R7_RAW=1`) — `panic in a function that cannot unwind` → `aborting`, код **134**.
- **segv** — процесс умирает: код **139**, строки `result:` нет.
- **abort** — процесс умирает: код **134**, результата нет.
- **oom** — при `ulimit -v 2000000`: `allocated chunk 29 (1920 MiB)` → `memory allocation of 67108864 bytes failed`, код **134**.
- **api_mismatch** — отбивается: `plugin_api=999, host expects 1`, `REFUSING to load plugin: ABI mismatch ...; plugin body was never called`, код **3**.
- **размер `.so`**: **551 608 B (539 KiB)** release, host 1 043 448 B.

## Каким путём пошли

abi_stable как задумано: `#[repr(C)] #[derive(StableAbi)] #[sabi(kind(Prefix(prefix_ref = RootModule_Ref)))]` + `export_root_module!` + `declare_root_module_statics!`, host — `RootModule_Ref::load_from_file`. `#[sabi_trait]` и ручной FFI не понадобились.

Но (a) не встроено: единственный `catch_unwind` в крейте — в загрузчике root-модуля (`src/library.rs:299`, даёт `RootModuleError::Unwound`), а `extern_fn_panic_handling!` документирован как «causes an abort if a panic reaches this point». `RResult` панику не ловит — нужен свой `catch_unwind` в точке входа.

## Доказательства

```
$ ./run.sh
== [1] ok ==                     result: Ok("pong:payload|method=7") / exit 0
== [2] panic ==                  result: Err("caught panic at ABI boundary: spike panic") / exit 0
== [3] panic R7_RAW=1 ==         panic in a function that cannot unwind / aborting / exit 134
== [4] abort ==                  exit 134, результата нет
== [5] segv ==                   exit 139, результата нет
== [6] oom ==                    memory allocation of 67108864 bytes failed / exit 134
== [7] api_mismatch ==           REFUSING to load plugin: ABI mismatch ... / exit 3
== [8] отдельная пересборка ==   host sha256 не изменился, новый плагин напечатал
                                 "pong:payload|v2_method=7", exit 0
== [9] исходники abi_stable ==   "This macro causes an abort if a panic reaches this point."
RESULT: all assertions passed
```

## Вывод для архитектуры

§6.2, пункт (a) сформулирован неверно: abi_stable не превращает панику в `Err` сам. Для `shell-plugin-host`/`shell-plugin-api`:

- каждый entry point (`init`/`call`/`shutdown`) оборачивать в `catch_unwind` внутри SDK-макроса; `RResult` необходим, но недостаточен;
- запретить `panic = "abort"` в профиле плагинов — иначе `catch_unwind` невозможен;
- паника до возврата `Err` всё равно идёт через panic hook и печатает backtrace — нужен свой `set_hook`;
- (b) подтверждён: segv/abort/oom убивают процесс и изнутри не видны; `Restart=always` + autodisable — единственная защита, счётчик падений должен опираться на факт смерти процесса (systemd `NRestarts`), а не на данные плагина;
- проверка `plugin_api` до вызова работает — оставить.

## Что осталось непроверенным

- реальный `Restart=always` и правило «3 падения за 5 минут» (юнита нет);
- многопоточный tokio-процесс: паника в рабочем потоке vs main;
- UB без segv — процесс живёт с испорченной памятью;
- `dlclose`/перезагрузка после падения, несовпадение версий abi_stable, target-feature;
- `extern "C-unwind"` + `catch_unwind` на стороне хоста;
- OOM через `ulimit -v` ≠ реальный OOM-killer.
