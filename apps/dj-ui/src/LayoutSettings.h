// The layout of the shell, as numbers a user can change (DJP-UI-001).
//
// The arrangement is not hard-coded: every region's share of the window is a setting, the presets are
// measured from a reference interface, and a dragged splitter is written back to the user's own file
// so the next start looks the way the last one was left.
//
// It lives here rather than in the engine's settings on purpose: the audio process has no business
// knowing where a panel sits, and a window layout must never be able to fail an audio start.

#pragma once

#include <QObject>
#include <QString>

class LayoutSettings : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString preset READ preset NOTIFY changed)
    Q_PROPERTY(int deckRows READ deckRows WRITE setDeckRows NOTIFY changed)
    Q_PROPERTY(double globalPercent READ globalPercent WRITE setGlobalPercent NOTIFY changed)
    // Derived: the deck rows take whatever the global strip and the browser leave. Setting an
    // independent number here would only produce a gap or an overflow.
    Q_PROPERTY(double deckRowsPercent READ deckRowsPercent NOTIFY changed)
    Q_PROPERTY(double browserPercent READ browserPercent WRITE setBrowserPercent NOTIFY changed)
    Q_PROPERTY(double mixerPercent READ mixerPercent WRITE setMixerPercent NOTIFY changed)
    Q_PROPERTY(QString channelOrder READ channelOrder WRITE setChannelOrder NOTIFY changed)
    Q_PROPERTY(QString file READ file NOTIFY changed)

public:
    explicit LayoutSettings(QObject *parent = nullptr);

    /// Presets, measured from the reference screenshots (see docs/qa/evidence/A07/layout.md).
    Q_INVOKABLE bool applyPreset(const QString &name);
    QString preset() const { return m_preset; }

    int deckRows() const { return m_deckRows; }
    void setDeckRows(int rows);
    double globalPercent() const { return m_globalPercent; }
    void setGlobalPercent(double percent);
    double deckRowsPercent() const { return m_deckRowsPercent; }
    double browserPercent() const { return m_browserPercent; }
    void setBrowserPercent(double percent);
    double mixerPercent() const { return m_mixerPercent; }
    void setMixerPercent(double percent);

    // The order of channel strips in the mixer, left to right, e.g. "CABD".
    QString channelOrder() const { return m_channelOrder; }
    bool setChannelOrder(const QString &order);

    QString file() const { return m_file; }
    void setFile(const QString &path);

    /// Read the file if it exists; a missing or broken file leaves the preset in place.
    Q_INVOKABLE bool load();
    /// Write the current numbers; called when a splitter is dropped.
    Q_INVOKABLE bool save() const;

    static QString defaultFile();

signals:
    void changed();

private:
    // Clamp to what the shell can draw and keep the three row shares inside one window.
    void normalise();

    QString m_preset = QStringLiteral("traktor");
    int m_deckRows = 2;
    double m_globalPercent = 8.0;
    double m_deckRowsPercent = 63.5;
    double m_browserPercent = 28.5;
    double m_mixerPercent = 14.5;
    QString m_channelOrder = QStringLiteral("CABD");
    QString m_file;
};
