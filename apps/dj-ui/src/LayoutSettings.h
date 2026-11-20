// The layout of the shell, as numbers a user can change (DJP-UI-001).
//
// The arrangement is not hard-coded: every region's share of the window is a setting, the presets are
// measured from a reference interface, and a dragged splitter is written back to the user's own file
// so the next start looks the way the last one was left.
//
// It lives here rather than in the engine's settings on purpose: the audio process has no business
// knowing where a panel sits, and a window layout must never be able to fail an audio start.

#pragma once

#include <QHash>
#include <QObject>
#include <QString>
#include <QStringList>
#include <QTimer>

class LayoutSettings : public QObject
{
    Q_OBJECT
    // Readable and writable: a dragged splitter leaves the named presets behind, and the shell
    // says so instead of pretending the arrangement is still "traktor".
    Q_PROPERTY(QString preset READ preset WRITE setPreset NOTIFY changed)
    // The arrangement is what the shell builds out of its regions, not just how much room each one
    // gets: which decks stand where, and which edge the mixer sits on. The numbers come per
    // arrangement, or a custom size in one layout would spoil another.
    Q_PROPERTY(QString arrangement READ arrangement NOTIFY changed)
    Q_PROPERTY(QString mixerEdge READ mixerEdge NOTIFY changed)
    Q_PROPERTY(int deckRows READ deckRows WRITE setDeckRows NOTIFY changed)
    Q_PROPERTY(double globalPercent READ globalPercent WRITE setGlobalPercent NOTIFY changed)
    // Derived: the deck rows take whatever the global strip and the browser leave. Setting an
    // independent number here would only produce a gap or an overflow.
    Q_PROPERTY(double deckRowsPercent READ deckRowsPercent NOTIFY changed)
    Q_PROPERTY(double browserPercent READ browserPercent WRITE setBrowserPercent NOTIFY changed)
    Q_PROPERTY(double mixerPercent READ mixerPercent WRITE setMixerPercent NOTIFY changed)
    Q_PROPERTY(QString channelOrder READ channelOrder WRITE setChannelOrder NOTIFY changed)
    // The browser's own numbers: how wide its rail of sections is and how large its text. They are
    // view sizes like any other, so they live with the layout and not in the folder the engine owns.
    Q_PROPERTY(int browserFont READ browserFont WRITE setBrowserFont NOTIFY changed)
    Q_PROPERTY(int browserRailWidth READ browserRailWidth WRITE setBrowserRailWidth NOTIFY changed)
    // The folders the browser's Collection section lists. One collection may live on several disks,
    // and each root holds its own tracks and its own .djmeta; empty means the default root.
    Q_PROPERTY(QStringList collectionRoots READ collectionRoots WRITE setCollectionRoots NOTIFY changed)
    /// The genres the collection is narrowed to: the chips the DJ left on, kept between runs.
    Q_PROPERTY(QStringList collectionGenres READ collectionGenres WRITE setCollectionGenres NOTIFY changed)
    // Visible fraction of a track in a deck's main waveform. Zero means "the deck's own default", so a
    // file that has never held a value still opens at the built-in zoom.
    Q_PROPERTY(double waveformSpan READ waveformSpan WRITE setWaveformSpan NOTIFY changed)
    /// How many beats the zoomed strip shows. A window in beats, not a fraction of the track: two decks
    /// at one tempo then show one picture, whatever their lengths (owner's decision, 2026-11-20).
    Q_PROPERTY(double waveformBeats READ waveformBeats WRITE setWaveformBeats NOTIFY changed)
    // The deck's modes that the DJ leaves behind: snap and quantize (global, in the mixer) and the scrub
    // mode (a setting). They live with the other view numbers so the next start comes back the way the
    // last one was left (owner's decision, 14.11).
    Q_PROPERTY(bool snapGrid READ snapGrid WRITE setSnapGrid NOTIFY changed)
    Q_PROPERTY(bool quantizeGrid READ quantizeGrid WRITE setQuantizeGrid NOTIFY changed)
    Q_PROPERTY(bool scrubAudible READ scrubAudible WRITE setScrubAudible NOTIFY changed)
    Q_PROPERTY(QString file READ file NOTIFY changed)
    // The collection's columns, in the DJ's order: a table he arranges once and finds again.
    Q_PROPERTY(QVariantList collectionColumns READ collectionColumns NOTIFY changed)

public:
    explicit LayoutSettings(QObject *parent = nullptr);

    /// Presets, measured from the reference screenshots (see docs/qa/evidence/A07/layout.md).
    Q_INVOKABLE bool applyPreset(const QString &name);
    /// Choose a layout by name: `classic`, `duo`, `stack-left`, `stack-right`, `stack-duo-left`,
    /// `stack-duo-right`, `studio-left`, `studio-right`, or `custom` (the numbers stay as they are).
    /// Old names `traktor` and `two-deck` are read as `classic` and `duo`.
    Q_INVOKABLE bool setLayout(const QString &name);
    QString preset() const { return m_preset; }
    QString arrangement() const { return m_arrangement; }
    QString mixerEdge() const { return m_mixerEdge; }
    // Only the name changes: "custom" keeps whatever numbers the hand produced.
    void setPreset(const QString &name);

    int deckRows() const { return m_deckRows; }
    void setDeckRows(int rows);
    double globalPercent() const { return m_globalPercent; }
    void setGlobalPercent(double percent);
    double deckRowsPercent() const { return m_deckRowsPercent; }
    double browserPercent() const { return m_browserPercent; }
    void setBrowserPercent(double percent);
    double mixerPercent() const { return m_mixerPercent; }
    void setMixerPercent(double percent);

    int browserFont() const { return m_browserFont; }
    void setBrowserFont(int pixels);
    int browserRailWidth() const { return m_browserRailWidth; }
    void setBrowserRailWidth(int pixels);
    QStringList collectionRoots() const { return m_collectionRoots; }
    QStringList collectionGenres() const { return m_collectionGenres; }
    void setCollectionRoots(const QStringList &roots);
    /// Narrow the collection to these genres. An empty list is the whole collection.
    void setCollectionGenres(const QStringList &genres);
    /// The root with the most free space: an import goes where there is room for it.
    Q_INVOKABLE QString roomiestRoot() const;
    /// The collection's columns in the DJ's order: `[{id, width}]`, the width a share of the list.
    QVariantList collectionColumns() const;
    Q_INVOKABLE void setCollectionColumnWidth(const QString &id, double width);
    Q_INVOKABLE void moveCollectionColumn(const QString &id, int toIndex);
    Q_INVOKABLE void resetCollectionColumns();
    double waveformSpan() const { return m_waveformSpan; }
    double waveformBeats() const { return m_waveformBeats; }
    void setWaveformBeats(double beats);
    bool snapGrid() const { return m_snapGrid; }
    void setSnapGrid(bool on);
    bool quantizeGrid() const { return m_quantizeGrid; }
    void setQuantizeGrid(bool on);
    bool scrubAudible() const { return m_scrubAudible; }
    void setScrubAudible(bool on);
    void setWaveformSpan(double span);

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
    // Absolute and unique: what a root list has to be before it is stored or written.
    static QStringList normalisedRoots(const QStringList &roots);

    // Clamp to what the shell can draw and keep the three row shares inside one window.
    void normalise();
    /// Keep the known columns, one of each, in a sane width range; a column the file forgot joins at
    /// the end with its default share.
    void normaliseColumns();

    // A dragged splitter emits a value per mouse move: the file is written once the hand stops.
    void scheduleSave();

    /// One layout's numbers: what the regions take and how many deck rows it shows.
    struct Numbers {
        int deckRows = 2;
        double globalPercent = 8.0;
        double browserPercent = 28.5;
        double mixerPercent = 14.5;
    };
    /// Where a preset name points: the arrangement it builds and the edge its mixer sits on.
    static bool parseLayout(const QString &name, QString *arrangement, QString *edge);
    /// The name this build writes for that arrangement and edge: `traktor` is read, but never stored.
    static QString canonicalName(const QString &arrangement, const QString &edge);
    /// The numbers a layout starts with before the DJ drags anything in it.
    static Numbers defaultsFor(const QString &arrangement);
    /// Keep the numbers of the layout being left, then take the ones of the layout being entered.
    void storeNumbers();
    void takeNumbers(const QString &arrangement);
    /// The numbers of every layout, by arrangement; the active one is written back on every switch.
    QHash<QString, Numbers> m_numbers;
    QString m_arrangement = QStringLiteral("classic");
    QString m_mixerEdge = QStringLiteral("centre");

    QString m_preset = QStringLiteral("traktor");
    int m_deckRows = 2;
    double m_globalPercent = 8.0;
    double m_deckRowsPercent = 63.5;
    double m_browserPercent = 28.5;
    double m_mixerPercent = 14.5;
    QString m_channelOrder = QStringLiteral("CABD");
    int m_browserFont = 11;
    int m_browserRailWidth = 92;
    QStringList m_collectionRoots;
    QStringList m_collectionGenres;
    /// The collection's columns, in order: id and its share of the list. These are the defaults; a
    /// saved arrangement replaces them, and a column the file forgot joins at the end.
    QVector<QPair<QString, double>> m_columns = {
        {QStringLiteral("artist"), 0.22}, {QStringLiteral("album"), 0.20},
        {QStringLiteral("title"), 0.30},  {QStringLiteral("time"), 0.08},
        {QStringLiteral("bpm"), 0.07},    {QStringLiteral("key"), 0.05},
        {QStringLiteral("recent"), 0.05},
    };
    double m_waveformSpan = 0.0;
    double m_waveformBeats = 16.0;
    // The deck's modes, on by default as the deck switches and the mixer's were.
    bool m_snapGrid = true;
    bool m_quantizeGrid = true;
    bool m_scrubAudible = false;
    QTimer *m_saveTimer = nullptr;
    QString m_file;
};
