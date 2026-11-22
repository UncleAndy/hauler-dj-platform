#!/usr/bin/env python3
"""Отметить требование проверенным — или посмотреть, что ждёт проверки.

`review` в `docs/requirements/traceability.csv` не значит «сделано»: это значит «реализация и свидетельство есть,
нужен человек». Этим скриптом человек и ставит свой вердикт: он не даёт отметить строку, которая не в `review`, не
придумывает дату за вас и печатает, что было и что стало.

    python3 scripts/verify-requirement.py --list
    python3 scripts/verify-requirement.py DJP-MIX-006 "Андрей Великоредчанин" [ГГГГ-ММ-ДД]

Запятых в имени быть не должно: гейт документации считает поля через awk -F, и запятая в тексте ломает и подсчёт
полей, и чтение статуса.
"""
import csv
import datetime
import sys
from pathlib import Path

CSV = Path(__file__).resolve().parent.parent / 'docs/requirements/traceability.csv'


def load(path: Path) -> list[dict]:
    with path.open(encoding='utf-8') as handle:
        return list(csv.DictReader(handle))


def save(path: Path, rows: list[dict]) -> None:
    fields = list(rows[0].keys())
    with path.open('w', encoding='utf-8', newline='') as handle:
        writer = csv.DictWriter(handle, fieldnames=fields, lineterminator='\n')
        writer.writeheader()
        writer.writerows(rows)


def main(argv: list[str]) -> int:
    if not argv or argv[0] in ('-h', '--help'):
        print(__doc__)
        return 0
    rows = load(CSV)
    if argv[0] == '--list':
        waiting = [row for row in rows if row['test_status'] == 'review']
        if not waiting:
            print('в review ничего нет')
            return 0
        print(f'ждут вердикта: {len(waiting)}')
        for row in waiting:
            print(f"  {row['req_id']:13} {row['test_id'][:44]:44} {row['evidence_link']}")
        return 0
    if argv[0] == '--defer':
        # Формальное исключение: требование известно, но в объём не входит — решением владельца, с причиной в строке.
        if len(argv) < 3:
            print('нужно: --defer "<причина>" "<имя владельца>" [--yes]', file=sys.stderr)
            return 2
        reason, owner = argv[1].strip(), argv[2].strip()
        confirmed = '--yes' in argv[3:]
        pending = [row for row in rows if row['test_status'] == 'unverified']
        if not pending:
            print('в unverified ничего нет')
            return 0
        print(f'в unverified {len(pending)} строк; будут помечены deferred с причиной «{reason}»:')
        for row in pending:
            print(f"  {row['req_id']:13} {row['implementation_path'][:70]}")
        if not confirmed:
            print('\nэто была проверка: ничего не изменено. Повторите с --yes.')
            return 0
        if ',' in reason or ',' in owner:
            print('в причине и имени не должно быть запятых', file=sys.stderr)
            return 2
        date = datetime.date.today().isoformat()
        for row in pending:
            row['test_status'] = 'deferred'
            row['reviewer'] = owner
            row['date'] = date
            if not row['implementation_path'].startswith('deferred:'):
                row['implementation_path'] = f'deferred: {reason}; ' + row['implementation_path']
        save(CSV, rows)
        print(f'\nпомечено deferred: {len(pending)}, владелец {owner}, {date}')
        return 0
    if argv[0] == '--batch-gate':
        # Массовая отметка того, что проверяется гейтом: cargo-тесты и скрипты. Список печатается целиком и только
        # после подтверждения меняется файл — ревьюер видит, за что расписывается, и не может «не заметить».
        if len(argv) < 2:
            print('нужно: --batch-gate "<имя ревьюера>" --yes [дата]', file=sys.stderr)
            return 2
        reviewer = argv[1].strip()
        confirmed = '--yes' in argv[2:]
        date = datetime.date.today().isoformat()
        covered = [
            row for row in rows
            if row['test_status'] == 'review'
            and (row['test_id'].startswith('cargo test') or row['test_id'].startswith('bash scripts/')
                 or row['test_id'].startswith('scripts/') or row['test_id'].startswith('ui.sh'))
        ]
        if not covered:
            print('в review нет строк, покрытых гейтом')
            return 0
        print(f'гейт покрывает {len(covered)} строк из review:')
        for row in covered:
            print(f"  {row['req_id']:13} {row['test_id'][:60]}")
        if not confirmed:
            print('\nэто была проверка: ничего не изменено. Повторите с --yes, если прогон гейта был зелёным.')
            return 0
        if ',' in reviewer:
            print('в имени не должно быть запятых', file=sys.stderr)
            return 2
        for row in covered:
            row['test_status'] = 'verified'
            row['reviewer'] = reviewer
            row['date'] = date
        save(CSV, rows)
        print(f"\nотмечено verified: {len(covered)}, ревьюер {reviewer}, {date}")
        return 0
    if len(argv) < 2:
        print('нужно: <DJP-...> "<имя ревьюера>" [дата]', file=sys.stderr)
        return 2
    wanted, reviewer = argv[0].strip(), argv[1].strip()
    date = argv[2].strip() if len(argv) > 2 else datetime.date.today().isoformat()
    if ',' in reviewer or ',' in date:
        print('в имени и дате не должно быть запятых: они ломают гейт документации', file=sys.stderr)
        return 2
    for row in rows:
        if row['req_id'] != wanted:
            continue
        if row['test_status'] != 'review':
            print(f"{wanted}: статус {row['test_status']}, а не review — отмечать нечего", file=sys.stderr)
            return 1
        row['test_status'] = 'verified'
        row['reviewer'] = reviewer
        row['date'] = date
        save(CSV, rows)
        print(f"{wanted}: review -> verified, ревьюер {reviewer}, {date}")
        print(f"  тест:          {row['test_id']}")
        print(f"  свидетельство: {row['evidence_link']}")
        return 0
    print(f'{wanted}: такого требования нет в трассировке', file=sys.stderr)
    return 1


if __name__ == '__main__':
    raise SystemExit(main(sys.argv[1:]))
