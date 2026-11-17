import json, math, sys, unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from speaker_identity import Identities

class IdentityTests(unittest.TestCase):
    def test_labels_can_swap_between_windows_without_swapping_people(self):
        tracker=Identities()
        first=tracker.assign({'S01':[1,0,0],'S02':[0,1,0]},1)
        tracker=Identities(json.loads(json.dumps(tracker.profiles)))
        later=tracker.assign({'S01':[0,1,0.01],'S02':[1,0.01,0]},2)
        self.assertEqual(first['S01'][0],later['S02'][0])
        self.assertEqual(first['S02'][0],later['S01'][0])
        self.assertNotEqual(later['S01'][0],later['S02'][0])
    def test_ambiguous_or_short_voice_is_never_forced_into_an_identity(self):
        tracker=Identities()
        tracker.assign({'S01':[1,0,0],'S02':[0,1,0]},1)
        later=tracker.assign({'S09':[1,1,0],'S10':None},2)
        self.assertTrue(all(v[0].startswith('uncertain-2-') for v in later.values()))
        self.assertEqual(len(tracker.profiles),2)
    def test_new_voice_and_user_merged_voice_profiles_remain_distinct(self):
        tracker=Identities([{'id':'p1','embedding':[1,0,0],'count':1},{'id':'p1','embedding':[0,1,0],'count':1}])
        voices=tracker.assign({'S01':[0,1,0],'S02':[0,0,1]},3)
        self.assertEqual(voices['S01'][0],'p1')
        self.assertNotEqual(voices['S02'][0],'p1')
        self.assertTrue(all(math.isfinite(v) for p in tracker.profiles for v in p['embedding']))
    def test_same_window_labels_do_not_silently_merge_even_with_similar_voices(self):
        tracker=Identities();tracker.assign({'S01':[1,0,0]},1)
        later=tracker.assign({'S09':[1,0,0],'S10':[1,.01,0]},2)
        self.assertNotEqual(later['S09'][0],later['S10'][0])

if __name__=='__main__':unittest.main()
