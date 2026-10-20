import sys, os, glob
sys.path.insert(0, os.path.dirname(__file__))
import spk, numpy as np
files = sorted(sum([glob.glob(a) for a in sys.argv[1:]], []))
E = [spk.emb(f) for f in files]
names = [os.path.basename(f)[:-4] for f in files]
w = max(len(n) for n in names)
print(" " * w, " ".join(f"{i:>4}" for i in range(len(names))))
for i, n in enumerate(names):
    st = spk.stats(files[i])
    print(f"{n:<{w}}", " ".join(f"{spk.cos(E[i], E[j]):4.2f}" for j in range(len(names))), f"  [{i}] {st}")
