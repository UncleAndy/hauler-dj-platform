// Separators, both spellings, on one machine.
//
// Every path bug this project has had on Windows came from a rule written with one separator and used where the
// other one arrives: a file name taken from a split on `/`, a folder recognised by a segment that was never found,
// a root compared with rows spelled the other way. The rules live in one place now (`PathRules.h`), and this check
// holds them to both spellings — on Linux, because the *rule* must not depend on the machine that runs it.

#include "Checks.h"

#include "../PathRules.h"

#include <QDir>
#include <QString>
#include <QStringList>

#include <cstdio>

namespace checks {

namespace {

int failures = 0;

void require(bool held, const QString &what)
{
    if (held) {
        std::fprintf(stdout, "dj-ui: %s\n", qPrintable(what));
        return;
    }
    ++failures;
    std::fprintf(stderr, "dj-ui: path rule failed: %s\n", qPrintable(what));
}

} // namespace

int runPathsCheck()
{
    const QString windows = QStringLiteral("C:\\Users\\uncle\\Music\\collection\\_duplicates\\A\\I\\x.flac");
    const QString windowsSlashes = QStringLiteral("C:/Users/uncle/Music/collection/_duplicates/A/I/x.flac");
    const QString posix = QStringLiteral("/home/andy/Music/collection/_duplicates/a/i/x.flac");

    require(pathrules::name(windows) == QStringLiteral("x.flac"),
            QStringLiteral("a name is taken from a backslash path"));
    require(pathrules::name(windowsSlashes) == QStringLiteral("x.flac"),
            QStringLiteral("a name is taken from a forward-slash path"));
    require(pathrules::name(posix) == QStringLiteral("x.flac"),
            QStringLiteral("a name is taken from a POSIX path"));
    require(pathrules::name(QStringLiteral("x.flac")) == QStringLiteral("x.flac"),
            QStringLiteral("a bare name is its own name"));
    require(pathrules::name(QString()) .isEmpty(), QStringLiteral("an empty path has no name"));

    require(pathrules::insideDuplicates(windows), QStringLiteral("a backslash path finds _duplicates"));
    require(pathrules::insideDuplicates(windowsSlashes),
            QStringLiteral("a forward-slash path finds _duplicates"));
    require(pathrules::insideDuplicates(posix), QStringLiteral("a POSIX path finds _duplicates"));
    require(!pathrules::insideDuplicates(QStringLiteral("C:\\Users\\uncle\\Music\\_duplicates_backup")),
            QStringLiteral("a folder that merely starts with the word is not the folder"));

    require(pathrules::segments(windows).size() == 9, QStringLiteral("a backslash path has nine parts"));
    require(pathrules::segments(windows) == pathrules::segments(windowsSlashes),
            QStringLiteral("both spellings of one path give the same parts"));

    // The root of a collection is spelled once per platform, and the trailing separator never survives: a root with
    // one is a root that does not compare equal to what the engine holds.
    const QString spelled = pathrules::spelled(QStringLiteral("C:/Users/uncle/Music/collection/"));
    require(spelled == QDir::toNativeSeparators(QStringLiteral("C:/Users/uncle/Music/collection")),
            QStringLiteral("a root loses its trailing separator and takes the platform's spelling"));
    require(pathrules::spelled(QStringLiteral("/home/andy/Music/")) == QStringLiteral("/home/andy/Music"),
            QStringLiteral("a POSIX root loses its trailing slash"));
    require(pathrules::spelled(QStringLiteral("/")) == QStringLiteral("/"),
            QStringLiteral("the root of a filesystem stays itself"));

    if (failures > 0) {
        std::fprintf(stderr, "dj-ui: %d path rule(s) failed\n", failures);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: path rules hold for both spellings: a name, a folder, the parts of a path, a root\n");
    return 0;
}

} // namespace checks
