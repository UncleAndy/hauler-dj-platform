#pragma once

// A crash that leaves nothing behind is a crash that has to be guessed at. Under Windows the shell dies on an
// access violation without a word: the log ends mid-sentence and the window is gone. This installs a last-chance
// writer that explains itself — the exception code, the address, and the module and offset of every frame — so the
// next report names the place instead of describing the silence.
void installCrashLog();

// Called from the ordinary exit path: a journal that ends with this line is a journal of a program that was closed,
// and a journal without it belongs to a program that died.
void noteShutdown();
