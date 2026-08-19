import json, subprocess, time, os, sys
from concurrent.futures import ThreadPoolExecutor

names = json.load(open('testdata/random_sample.json'))
manifest = {m['name']: m for m in json.load(open('testdata/dbtests2/manifest.json'))}

def run_rust(name):
    subprocess.run(['./target/release/osu_replay_judge.exe',
                    f'testdata/dbtests2/{name}.osu', f'testdata/dbtests2/{name}.osr',
                    '--timeline', f'trs_{name}.tsv'],
                   capture_output=True, timeout=120)
    return name

with ThreadPoolExecutor(max_workers=4) as ex:
    list(ex.map(run_rust, names))
print("rust timelines done", flush=True)

for i, name in enumerate(names):
    m = manifest[name]
    gt = os.path.abspath(f'gts_{name}.tsv')
    if os.path.exists(gt): os.remove(gt)
    env = dict(os.environ,
               OSU_JUDGE_MAP=os.path.abspath(f'testdata/dbtests2/{name}.osu'),
               OSU_JUDGE_REPLAY=os.path.abspath(f'testdata/dbtests2/{name}.osr'),
               OSU_JUDGE_OUT=gt)
    if m['is_legacy']:
        env['OSU_JUDGE_CLASSIC'] = '1'
    else:
        env.pop('OSU_JUDGE_CLASSIC', None)
    t0 = time.time()
    r = subprocess.run(['dotnet', 'test', 'D:/Github/osu/osu.Game.Rulesets.Osu.Tests/osu.Game.Rulesets.Osu.Tests.csproj',
                        '-c', 'Debug', '--filter', 'FullyQualifiedName~TestSceneReplayJudgeHarness.RunReplay',
                        '--nologo', '--nologo'],
                       capture_output=True, encoding='utf-8', errors='replace', env=env,
                       cwd='D:/Github/osu', timeout=1500)
    dt = time.time() - t0
    n_lines = sum(1 for _ in open(gt, encoding='utf-8', errors='replace')) if os.path.exists(gt) else 0
    print(f"[{i+1}/{len(names)}] {name} legacy={m['is_legacy']} gt_events={n_lines} ({dt:.0f}s)", flush=True)
