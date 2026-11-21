#pragma once

#include <QObject>
#include <QVariant>
#include <QVariantMap>

class EngineSettings;
class LayoutSettings;
class SessionState;
class UiSettings;

// One place that knows which file a setting lives in (stage 0 of the settings window, 21.11).
//
// The window must not write anything itself: three owners already exist (layout.json, session.json, the shell's own
// UiSettings) and the engine's config.toml gets a fourth writer only through the engine. This class routes by the
// prefix of a path — `engine:audio.block_frames`, `layout:deckRows`, `session:crossfaderCurve`, `ui:frameHz` — and
// nothing else. A setting the owner does not have is refused, not invented: a control nobody reads is a lie.
class SettingsModel : public QObject
{
    Q_OBJECT

public:
    SettingsModel(LayoutSettings &layout, UiSettings &ui, SessionState &session, EngineSettings &engine,
                  QObject *parent = nullptr);

    /// The value as the owner has it; an invalid answer means "this setting does not exist".
    Q_INVOKABLE QVariant value(const QString &path) const;

    /// Write one value through its owner. `false` when the path is unknown or the owner refused it.
    Q_INVOKABLE bool set(const QString &path, const QVariant &value);

    /// True when the engine has to restart before the value is in force.
    Q_INVOKABLE bool needsRestart(const QString &path) const;

    /// Which file the value lands in, for the window's hint under a control.
    Q_INVOKABLE QString fileOf(const QString &path) const;

    /// The deck's effect, as `session:deckFx.A` names it.
    Q_INVOKABLE QString deckFx(const QString &deck) const;

    /// A short report for the clipboard: versions, the files, the audio shape.
    Q_INVOKABLE QString report() const;

signals:
    void changed();

private:
    QObject *ownerOf(const QString &path, QString *name) const;

    LayoutSettings &m_layout;
    UiSettings &m_ui;
    SessionState &m_session;
    EngineSettings &m_engine;
};

