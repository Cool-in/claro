"""Build the CPU-only native offline engine. Python is build tooling only."""
import argparse, os, pathlib, platform, shutil, subprocess
parser=argparse.ArgumentParser()
parser.add_argument('--cache',required=True,type=pathlib.Path)
parser.add_argument('--jobs',type=int,default=4)
args=parser.parse_args()
cache=args.cache.resolve();cache.mkdir(parents=True,exist_ok=True)
repo=pathlib.Path(__file__).resolve().parents[1]
windows=platform.system()=='Windows'
def run(command,cwd=None):
    print(' '.join(map(str,command)),flush=True)
    subprocess.run(list(map(str,command)),cwd=cwd,check=True)
def checkout(name,url,tag,commit):
    path=cache/name
    if not path.exists():run(['git','clone','--depth','1','--branch',tag,url,path])
    actual=subprocess.check_output(['git','rev-parse','HEAD'],cwd=path,text=True).strip()
    if actual!=commit:raise RuntimeError(f'{name} cache has another revision; use the expected pinned checkout.')
    return path
ct2=checkout('ctranslate2-source','https://github.com/OpenNMT/CTranslate2.git','v4.8.4','5de572ef60b67a0276c5989ee73d0072808ff4c3')
sp=checkout('sentencepiece-source','https://github.com/google/sentencepiece.git','v0.2.1','31646a467d2051eb904e0b45de3a73e91fe1c1e3')
run(['git','submodule','update','--init','--recursive','third_party/spdlog','third_party/cpu_features','third_party/ruy'],ct2)
build=cache/'offline-native'
ct2build=build/'ctranslate2';spbuild=build/'sentencepiece';runnerbuild=build/'runner'
generator=['-G','Ninja'] if windows and shutil.which('ninja') else []
ct2args=['-DWITH_MKL=OFF','-DWITH_CUDA=OFF','-DWITH_CUDNN=OFF','-DWITH_RUY=ON','-DOPENMP_RUNTIME=NONE','-DBUILD_CLI=OFF','-DBUILD_TESTS=OFF','-DBUILD_SHARED_LIBS=ON']
if not windows:ct2args+=['-DWITH_ACCELERATE=ON','-DCMAKE_OSX_DEPLOYMENT_TARGET=13.0']
run(['cmake','-S',ct2,'-B',ct2build,*generator,'-DCMAKE_BUILD_TYPE=Release','-DCMAKE_POLICY_VERSION_MINIMUM=3.5',*ct2args])
run(['cmake','--build',ct2build,'--config','Release','--parallel',args.jobs])
run(['cmake','-S',sp,'-B',spbuild,*generator,'-DCMAKE_BUILD_TYPE=Release','-DCMAKE_POLICY_VERSION_MINIMUM=3.5','-DSPM_ENABLE_SHARED=OFF','-DSPM_BUILD_TEST=OFF','-DSPM_ENABLE_NFKC_COMPILE=OFF'])
run(['cmake','--build',spbuild,'--config','Release','--parallel',args.jobs,'--target','sentencepiece-static'])
ct2lib=next(ct2build.rglob('ctranslate2.lib' if windows else 'libctranslate2.dylib'))
splib=next(spbuild.rglob('sentencepiece.lib' if windows else 'libsentencepiece.a'))
run(['cmake','-S',repo/'offline','-B',runnerbuild,*generator,'-DCMAKE_BUILD_TYPE=Release',f'-DCTRANSLATE2_INCLUDE={ct2}/include',f'-DCTRANSLATE2_LIBRARY={ct2lib}',f'-DSENTENCEPIECE_INCLUDE={sp}/src',f'-DSENTENCEPIECE_LIBRARY={splib}'])
run(['cmake','--build',runnerbuild,'--config','Release','--parallel',args.jobs])
executable=next(runnerbuild.rglob('claro-offline.exe' if windows else 'claro-offline'))
if executable.parent!=runnerbuild:shutil.copy2(executable,runnerbuild/executable.name)
if windows:
    shutil.copy2(next(ct2build.rglob('ctranslate2.dll')),runnerbuild/'ctranslate2.dll')
else:
    library=runnerbuild/'libctranslate2.dylib'
    shutil.copy2(ct2lib.resolve(),library)
    run(['install_name_tool','-id','@executable_path/libctranslate2.dylib',library])
    dependencies=subprocess.check_output(['otool','-L',runnerbuild/'claro-offline'],text=True)
    for line in dependencies.splitlines()[1:]:
        name=line.strip().split(' (')[0]
        if 'libctranslate2' in name:run(['install_name_tool','-change',name,'@executable_path/libctranslate2.dylib',runnerbuild/'claro-offline'])
    identity=os.environ.get('CLARO_SIGN_IDENTITY','-')
    for artifact in [library,runnerbuild/'claro-offline']:
        command=['codesign','--force','--sign',identity]
        if identity!='-':command+=['--options','runtime','--timestamp']
        run([*command,artifact])
print(f'CLARO_OFFLINE_DIR={runnerbuild}',flush=True)
