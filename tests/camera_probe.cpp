// Reads compressed video samples for timing only; never saves camera images.
#include <windows.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <wrl/client.h>
#include <chrono>
#include <cstdio>
#include <cwchar>
#include <stdexcept>
#include <atomic>
#include <thread>
using Microsoft::WRL::ComPtr;
static std::atomic<unsigned long long> samples_received{0};
static void check(HRESULT result) {
    if (FAILED(result)) {
        std::fprintf(stderr,"Media Foundation failure: 0x%08lx\n",static_cast<unsigned long>(result));
        throw std::runtime_error("camera operation failed");
    }
}
int wmain(int argc,wchar_t **argv) {
    try {
        const int seconds=argc>1?_wtoi(argv[1]):60;
        if(seconds<1||seconds>3600) return 2;
        check(CoInitializeEx(nullptr,COINIT_MULTITHREADED));
        check(MFStartup(MF_VERSION));
        ComPtr<IMFAttributes> attributes;
        check(MFCreateAttributes(&attributes,1));
        check(attributes->SetGUID(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID));
        IMFActivate **devices=nullptr;
        UINT32 count=0;
        check(MFEnumDeviceSources(attributes.Get(),&devices,&count));
        ComPtr<IMFMediaSource> source;
        for(UINT32 index=0;index<count;++index) {
            wchar_t *link=nullptr;
            UINT32 length=0;
            if(SUCCEEDED(devices[index]->GetAllocatedString(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,&link,&length))) {
                if(wcsstr(link,L"vid_1bcf&pid_0b40")||wcsstr(link,L"VID_1BCF&PID_0B40"))
                    check(devices[index]->ActivateObject(IID_PPV_ARGS(&source)));
                CoTaskMemFree(link);
            }
            devices[index]->Release();
        }
        CoTaskMemFree(devices);
        if(!source) throw std::runtime_error("1bcf:0b40 camera not found");
        ComPtr<IMFSourceReader> reader;
        check(MFCreateSourceReaderFromMediaSource(source.Get(),nullptr,&reader));
        ComPtr<IMFMediaType> selected;
        UINT32 width=0,height=0,rate=0,denominator=1;
        for(DWORD index=0;;++index) {
            ComPtr<IMFMediaType> type;
            const HRESULT result=reader->GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM,index,&type);
            if(result==MF_E_NO_MORE_TYPES) break;
            check(result);
            GUID subtype{};
            UINT32 w=0,h=0,n=0,d=1;
            if(FAILED(type->GetGUID(MF_MT_SUBTYPE,&subtype))||FAILED(MFGetAttributeSize(type.Get(),MF_MT_FRAME_SIZE,&w,&h))||FAILED(MFGetAttributeRatio(type.Get(),MF_MT_FRAME_RATE,&n,&d))) continue;
            if(subtype==MFVideoFormat_MJPG&&w<=1920&&h<=1080&&d&&n/d>=25&&n/d<=30&&w*h>width*height) {
                selected=type;width=w;height=h;rate=n;denominator=d;
            }
        }
        if(!selected) throw std::runtime_error("No MJPEG mode at 25-30 fps up to 1920x1080");
        check(reader->SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM,nullptr,selected.Get()));
        check(reader->SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS,FALSE));
        check(reader->SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM,TRUE));
        std::printf("MJPEG %ux%u %.3f fps; timing samples only, no images saved\n",width,height,static_cast<double>(rate)/denominator);
        std::fflush(stdout);
        // Diagnostic-only watchdog: do not leave an unattended benchmark hung
        // inside a camera driver. No such deadline exists in the server/client.
        std::thread([seconds] {
            Sleep(20000);
            if(samples_received.load()==0) {
                std::fprintf(stderr,"No video sample arrived within 20 seconds\n");
                ExitProcess(3);
            }
            Sleep(static_cast<DWORD>((seconds+10)*1000));
            std::fprintf(stderr,"Camera probe exceeded its requested test duration\n");
            ExitProcess(4);
        }).detach();
        const auto start=std::chrono::steady_clock::now();
        auto report=start;
        unsigned long long frames=0,bytes=0;
        for(;;) {
            DWORD stream=0,flags=0;
            LONGLONG timestamp=0;
            ComPtr<IMFSample> sample;
            check(reader->ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM,0,&stream,&flags,&timestamp,&sample));
            if(flags&(MF_SOURCE_READERF_ERROR|MF_SOURCE_READERF_ENDOFSTREAM)) throw std::runtime_error("Camera stream ended unexpectedly");
            if(sample) {DWORD size=0;check(sample->GetTotalLength(&size));++frames;bytes+=size;samples_received.store(frames);}
            const auto now=std::chrono::steady_clock::now();
            const double elapsed=std::chrono::duration<double>(now-start).count();
            if(std::chrono::duration<double>(now-report).count()>=5||elapsed>=seconds) {
                std::printf("seconds=%.2f frames=%llu fps=%.3f payload_MiB_s=%.3f\n",elapsed,frames,frames/elapsed,bytes/elapsed/1048576.0);
                std::fflush(stdout);report=now;
            }
            if(elapsed>=seconds) break;
        }
        reader.Reset();source->Shutdown();source.Reset();
        MFShutdown();CoUninitialize();
        return frames>0?0:1;
    } catch(const std::exception &e) {std::fprintf(stderr,"%s\n",e.what());return 1;}
}
