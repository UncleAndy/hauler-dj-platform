// Scenario checks for the DJ shell (DJP-DECK-003/004 and the UI around them).
//
// Each case is a user story driven through the real widgets: the button is clicked, the strip is read
// through its own properties, and the facts are taken from the engine's snapshot. A case that cannot
// see what it is looking for fails with a sentence saying what it expected.

#pragma once

#include <QString>

class BridgeClient;
class QQuickWindow;

namespace djui {

/// Run every scenario case against a live engine and a shown window.
///
/// Returns 0 when all cases passed, 1 otherwise. Prints one line per case.
int runScenarios(BridgeClient &bridge, QQuickWindow &window, const QString &deck = QStringLiteral("A"));

} // namespace djui
