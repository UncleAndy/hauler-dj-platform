#pragma once

#include <QDir>
#include <QFileInfo>
#include <QString>
#include <QStringList>

// Paths are not one thing. A playlist folder is a logical path and always uses `/` on the wire; a file on disk is
// spelled the way the platform spells it, and on Windows both `/` and `\` are accepted by every API — which is
// exactly why a rule written with one of them works on Linux, works by accident on Windows, and fails where it
// matters: a name taken from a path, a folder recognised by its last segment, a root compared with a row.
namespace pathrules {

/// The segments of a filesystem path, whichever separator the path happens to use.
inline QStringList segments(const QString &path)
{
    QString normalised = path;
    normalised.replace(QLatin1Char('\\'), QLatin1Char('/'));
    return normalised.split(QLatin1Char('/'), Qt::SkipEmptyParts);
}

/// The last segment: the file or folder's own name, on either platform.
///
/// Built on `segments`, not on `QFileInfo`: a Windows path read on Linux is a string with backslashes in it, and
/// `QFileInfo` would hand back the whole path as the name. A rule that names a file must not depend on which
/// machine reads the name.
inline QString name(const QString &path)
{
    const QStringList parts = segments(path);
    return parts.isEmpty() ? QString() : parts.last();
}

/// A filesystem path in this platform's spelling, with no trailing separator.
inline QString spelled(const QString &path)
{
    QString one = QDir::toNativeSeparators(QDir::cleanPath(path));
    while (one.size() > 1 && (one.endsWith(QLatin1Char('/')) || one.endsWith(QLatin1Char('\\')))) {
        one.chop(1);
    }
    return one;
}

/// Whether a folder the import parks a rejected copy in is anywhere in this path.
inline bool insideDuplicates(const QString &directory)
{
    return segments(directory).contains(QStringLiteral("_duplicates"));
}

} // namespace pathrules
