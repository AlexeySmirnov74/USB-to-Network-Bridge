// DirectShow video timing probe. Counts samples; never saves images.
#include <windows.h>
#include <dshow.h>
#include <dvdmedia.h>
#include <wrl/client.h>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <stdexcept>
#include <thread>
#include <vector>
using Microsoft::WRL::ComPtr;
MIDL_INTERFACE("0579154A-2B53-4994-B0D0-E773148EFF85") ISampleGrabberCB:public IUnknown {
 virtual HRESULT STDMETHODCALLTYPE SampleCB(double,IMediaSample*)=0;
 virtual HRESULT STDMETHODCALLTYPE BufferCB(double,BYTE*,long)=0;
};
MIDL_INTERFACE("6B652FFF-11FE-4FCE-92AD-0266B5D7C78F") ISampleGrabber:public IUnknown {
 virtual HRESULT STDMETHODCALLTYPE SetOneShot(BOOL)=0;
 virtual HRESULT STDMETHODCALLTYPE SetMediaType(const AM_MEDIA_TYPE*)=0;
 virtual HRESULT STDMETHODCALLTYPE GetConnectedMediaType(AM_MEDIA_TYPE*)=0;
 virtual HRESULT STDMETHODCALLTYPE SetBufferSamples(BOOL)=0;
 virtual HRESULT STDMETHODCALLTYPE GetCurrentBuffer(long*,long*)=0;
 virtual HRESULT STDMETHODCALLTYPE GetCurrentSample(IMediaSample**)=0;
 virtual HRESULT STDMETHODCALLTYPE SetCallback(ISampleGrabberCB*,long)=0;
};
static const GUID Grabber={0xC1F400A0,0x3F08,0x11D3,{0x9F,0x0B,0x00,0x60,0x08,0x03,0x9E,0x37}};
static const GUID DiscardRenderer={0xC1F400A4,0x3F08,0x11D3,{0x9F,0x0B,0x00,0x60,0x08,0x03,0x9E,0x37}};
static void check(HRESULT result){if(FAILED(result)){std::fprintf(stderr,"DirectShow failure: 0x%08lx\n",static_cast<unsigned long>(result));throw std::runtime_error("camera operation failed");}}
class Counter final:public ISampleGrabberCB {
 std::atomic<ULONG> references{1};
public:
 std::atomic<unsigned long long> frames{0},bytes{0};
 HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id,void **value)override {if(id==IID_IUnknown||id==__uuidof(ISampleGrabberCB)){*value=static_cast<ISampleGrabberCB*>(this);AddRef();return S_OK;}*value=nullptr;return E_NOINTERFACE;}
 ULONG STDMETHODCALLTYPE AddRef()override{return ++references;}
 ULONG STDMETHODCALLTYPE Release()override {ULONG count=--references;if(!count)delete this;return count;}
 HRESULT STDMETHODCALLTYPE SampleCB(double,IMediaSample *sample)override {++frames;bytes+=sample->GetActualDataLength();return S_OK;}
 HRESULT STDMETHODCALLTYPE BufferCB(double,BYTE*,long)override{return E_NOTIMPL;}
};
static void free_type(AM_MEDIA_TYPE *type){if(!type)return;if(type->cbFormat)CoTaskMemFree(type->pbFormat);if(type->pUnk)type->pUnk->Release();CoTaskMemFree(type);}
int wmain(int argc,wchar_t **argv){try{
 const int seconds=argc>1?_wtoi(argv[1]):30;if(seconds<1||seconds>600)return 2;
 const int requested_width=argc>2?_wtoi(argv[2]):1920;
 const int requested_height=argc>3?_wtoi(argv[3]):1080;
 const wchar_t *requested_name=argc>4?argv[4]:L"2K FHD camera";
 check(CoInitializeEx(nullptr,COINIT_MULTITHREADED));
 ComPtr<ICreateDevEnum> enumeration;check(CoCreateInstance(CLSID_SystemDeviceEnum,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&enumeration)));
 ComPtr<IEnumMoniker> monikers;check(enumeration->CreateClassEnumerator(CLSID_VideoInputDeviceCategory,&monikers,0));
 if(!monikers)throw std::runtime_error("No video capture device");
 ComPtr<IBaseFilter> camera;
 for(ComPtr<IMoniker> moniker;monikers->Next(1,&moniker,nullptr)==S_OK;moniker.Reset()){
  ComPtr<IPropertyBag> properties;check(moniker->BindToStorage(nullptr,nullptr,IID_PPV_ARGS(&properties)));
  VARIANT name;VariantInit(&name);
  if(SUCCEEDED(properties->Read(L"FriendlyName",&name,nullptr))&&name.vt==VT_BSTR&&wcscmp(name.bstrVal,requested_name)==0)check(moniker->BindToObject(nullptr,nullptr,IID_PPV_ARGS(&camera)));
  VariantClear(&name);
 }
 if(!camera)throw std::runtime_error("Requested camera not found");
 ComPtr<IGraphBuilder> graph;check(CoCreateInstance(CLSID_FilterGraph,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&graph)));
 ComPtr<ICaptureGraphBuilder2> builder;check(CoCreateInstance(CLSID_CaptureGraphBuilder2,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&builder)));
 check(builder->SetFiltergraph(graph.Get()));check(graph->AddFilter(camera.Get(),L"USB camera"));
 ComPtr<IAMStreamConfig> configuration;check(builder->FindInterface(&PIN_CATEGORY_CAPTURE,&MEDIATYPE_Video,camera.Get(),IID_IAMStreamConfig,reinterpret_cast<void**>(configuration.GetAddressOf())));
 int count=0,capability_size=0;check(configuration->GetNumberOfCapabilities(&count,&capability_size));
 std::vector<BYTE> capability(capability_size);bool selected=false;
 for(int index=0;index<count;++index){
  AM_MEDIA_TYPE *type=nullptr;check(configuration->GetStreamCaps(index,&type,capability.data()));
  LONG width=0,height=0;REFERENCE_TIME interval=0;
  if(type->formattype==FORMAT_VideoInfo&&type->cbFormat>=sizeof(VIDEOINFOHEADER)){auto *info=reinterpret_cast<VIDEOINFOHEADER*>(type->pbFormat);width=info->bmiHeader.biWidth;height=info->bmiHeader.biHeight;interval=info->AvgTimePerFrame;}
  if(type->formattype==FORMAT_VideoInfo2&&type->cbFormat>=sizeof(VIDEOINFOHEADER2)){auto *info=reinterpret_cast<VIDEOINFOHEADER2*>(type->pbFormat);width=info->bmiHeader.biWidth;height=info->bmiHeader.biHeight;interval=info->AvgTimePerFrame;}
  if(!selected&&type->subtype==MEDIASUBTYPE_MJPG&&width==requested_width&&height==requested_height&&interval>=333000&&interval<=401000){check(configuration->SetFormat(type));selected=true;std::printf("DirectShow MJPEG %ldx%ld %.3f fps; no images saved\n",width,height,1e7/interval);std::fflush(stdout);}
  free_type(type);
 }
 if(!selected)throw std::runtime_error("Requested MJPEG mode at 25-30 fps not found");
 ComPtr<IBaseFilter> grabber_filter;check(CoCreateInstance(Grabber,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&grabber_filter)));
 ComPtr<ISampleGrabber> grabber;check(grabber_filter.As(&grabber));
 AM_MEDIA_TYPE media{};media.majortype=MEDIATYPE_Video;media.subtype=MEDIASUBTYPE_MJPG;check(grabber->SetMediaType(&media));
 ComPtr<Counter> counter;counter.Attach(new Counter());check(grabber->SetOneShot(FALSE));check(grabber->SetBufferSamples(FALSE));check(grabber->SetCallback(counter.Get(),0));
 check(graph->AddFilter(grabber_filter.Get(),L"Sample counter"));
 ComPtr<IBaseFilter> sink;check(CoCreateInstance(DiscardRenderer,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&sink)));check(graph->AddFilter(sink.Get(),L"Discard video"));
 check(builder->RenderStream(&PIN_CATEGORY_CAPTURE,&MEDIATYPE_Video,camera.Get(),grabber_filter.Get(),sink.Get()));
 ComPtr<IMediaFilter> media_filter;check(graph.As(&media_filter));check(media_filter->SetSyncSource(nullptr));
 ComPtr<IMediaControl> control;check(graph.As(&control));
 std::thread([seconds]{Sleep((seconds+20)*1000);std::fprintf(stderr,"DirectShow probe exceeded test duration\n");ExitProcess(4);}).detach();
 check(control->Run());const auto begin=std::chrono::steady_clock::now();
 for(int second=1;second<=seconds;++second){Sleep(1000);if(second%5==0||second==seconds){const double elapsed=std::chrono::duration<double>(std::chrono::steady_clock::now()-begin).count();std::printf("seconds=%.2f frames=%llu fps=%.3f payload_MiB_s=%.3f\n",elapsed,counter->frames.load(),counter->frames.load()/elapsed,counter->bytes.load()/elapsed/1048576.0);std::fflush(stdout);}}
 const bool passed=counter->frames.load()>0;check(control->Stop());return passed?0:3;
 }catch(const std::exception &error){std::fprintf(stderr,"%s\n",error.what());return 1;}}
