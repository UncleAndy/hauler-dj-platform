# Evidence

Формат (план §7.3): `docs/qa/evidence/<task-id>/`

```text
<task-id>/
├── summary.md        # что проверено, commit, окружение
├── test-results.txt  # реально запущенные команды и exit codes
├── benchmark.json    # если релевантно: latency, p99, xruns
├── fixtures/         # только собственные/разрешённые материалы
└── limitations.md    # unknown, excluded, not verified
```

Наличие файлов не означает «пройдено»: нужны фактические результаты и ревью человека.
Персональные пути и серийные номера перед публикацией обезличиваются.
