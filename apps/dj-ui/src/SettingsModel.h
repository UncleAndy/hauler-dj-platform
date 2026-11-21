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

    /// One channel of a pair setting (`audio.master`, `audio.headphones.pair`): the number, or -1 for "none".
    Q_INVOKABLE int channelAt(const QString &path, int side) const;

    /// Write one channel of a pair setting. A negative value means "no channel": the pair loses that side and the cue is
    /// switched off, because a pair with a hole in it is not a route.
    Q_INVOKABLE bool setChannelAt(const QString &path, int side, int channel);

    /// What the device is like — rate, bits, channels, sample format — as the engine sees it.
    Q_INVOKABLE QVariantMap deviceShape(const QString &device) const;

    /// Put the headphones into this mode: ask the engine what the mode means and store the answer.
    ///
    /// `auto` takes the main device and its spare channels, `same_device` keeps the main device and the DJ's choice where
    /// it is free, `device` leaves both to the DJ. When the interface has no spare pair the cue is switched off and the
    /// window shows "None" — the mixing still happens, it just has nowhere to go.
    Q_INVOKABLE bool applyHeadphoneMode(const QString &mode);

    /// What the headphones would get in this mode, decided by the engine: device, left, right (-1 for none).
    Q_INVOKABLE QVariantMap resolveCue(const QString &mode) const;

    /// The outputs the engine can open, as names to choose from rather than a string to guess.
    Q_INVOKABLE QStringList devices() const;

    /// The stereo pairs the device offers — the headphones are a pair or nothing.
    Q_INVOKABLE QStringList devicePairs(const QString &device) const;

    /// Ask the engine about one device: what it opens, or why it cannot.
    Q_INVOKABLE QString describeDevice(const QString &name) const;

    /// A short report for the clipboard: versions, the files, the audio shape.
    Q_INVOKABLE QString report() const;

    /// Put that report on the clipboard, for a message to whoever is helping.
    Q_INVOKABLE void copyReport() const;

signals:
    void changed();

private:
    QObject *ownerOf(const QString &path, QString *name) const;

    LayoutSettings &m_layout;
    UiSettings &m_ui;
    SessionState &m_session;
    EngineSettings &m_engine;
};

